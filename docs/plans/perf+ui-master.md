# Master plan — v1.1 perf + UI pass

**Status:** master only. Per-piece plans land in sibling files
(`docs/plans/perf-1-…md`, etc.). This file is the index, dependency
graph, and commit-ordering contract. Eight planner-agent siblings will
each consume one row of §2 and write a detailed plan; a cross-reviewer
will compare all eight against §6.

Source-of-truth docs (read first):
- `docs/research/perf-final-report.md` (§3 roadmap, §5 determinism, §6 commit shape)
- `docs/architecture.md` (tick ordering, module map)
- `BUILD-REPORT.md` (§16 status, known issues)
- `docs/plans/perf-timing.md` (the already-shipped profiler the UI pass
  must inherit cleanly)
- `DECISIONS.md` (running log)

---

## 1. Goals & non-goals

**Goals.** Land the five user-pinned perf wins from
`perf-final-report.md` and re-shape the right-hand UI into three
independently-visible widgets (Stats, Inspector, Perf). After this pass:
the §16 acceptance test still passes against the existing pinned
sequential golden (unchanged); Perf-4 adds a new threaded golden
exercising the rayon codepath (`tests/golden_snapshot_t10000_threaded.txt`)
under `#[cfg(feature="threads")]`; Stats charts and the profiler
table are always visible at viewport corners; the Inspector pops up
only on creature click; and there is no rail panel hiding the canvas
by default.

**Non-goals.** Anything outside v5 §14 "Included" — no new chart
library, no framework, no SIMD ray-vs-circle batch (perf research item
#8, deferred), no NN weight transpose (item #13, deferred), no
render-side polish items (#14–18), no re-enabling of events / event
log UI, no new sliders, no spec-edits to v5/v6/ORCHESTRATOR/archive
docs, no second Cargo crate. The profiler (perf-timing) is already
shipped; this pass only **relocates its table widget**, it does not
re-design it.

---

## 2. Inventory of pieces

| id | name | track | detailed plan file | risk | determinism |
|---|---|---|---|---|---|
| perf-1 | Pre-compute sector sin/cos per creature | perf | `docs/plans/perf-1-sector-trig.md` | low | safe |
| perf-2 | Pool per-tick scratch Vecs onto `World` | perf | `docs/plans/perf-2-scratch-vecs.md` | low | safe |
| perf-3 | Pre-allocate `SpatialGrid` cursor buffer | perf | `docs/plans/perf-3-grid-cursors.md` | low | safe |
| perf-4 | Threads end-to-end (JS init + vision parallelism) | perf | `docs/plans/perf-4-threads.md` | high | adds new golden (default build unchanged) |
| perf-5 | Genome hot-field SoA split (7 fields, mirror) | perf | `docs/plans/perf-5-genome-soa.md` | med | safe |
| ui-stats | Top-left Stats box (always visible, owns charts) | ui | `docs/plans/ui-stats-box.md` | low | safe |
| ui-inspector | Top-right Inspector pop-up (hidden by default) | ui | `docs/plans/ui-inspector-box.md` | low | safe |
| ui-perf | Bottom-left Perf box (always visible, owns profiler table) | ui | `docs/plans/ui-perf-box.md` | low | safe |

All eight pieces are 1 commit each. Perf-4 adds a **new** threaded
golden file (`tests/golden_snapshot_t10000_threaded.txt`) but leaves
the default-build sequential golden untouched. UI pieces touch zero
Rust code (see §3).

---

## 3. Dependency graph

```
perf-1 ─┐
perf-2 ─┼─► (any order, all golden-safe)
perf-3 ─┘
            │
            ▼
        perf-5  (golden-safe; lands as natural review checkpoint per
            │     perf-final-report §6 commit 5)
            ▼
        perf-4  (golden-safe for default build; adds a NEW threaded
                 golden file + a #[cfg(feature="threads")] acceptance
                 test + DECISIONS line. Recommended last in the perf
                 block for review-checkpoint clarity, but order is no
                 longer load-bearing for bisectability.)

ui-stats ────► (independent of all perf; can interleave anywhere)
ui-inspector ► (independent of all perf; can interleave anywhere)
ui-perf ─────► (independent of all perf; can interleave anywhere)
```

**Key dependency findings (confirmed by reading source):**

- **Perf-5 vs UI-Inspector.** `creature_inspect_json`
  (`src/wasm_api.rs:243–280`) reads from `self.inner.creatures.genomes[i]`
  via the existing `genomes: Vec<Genome>` AoS path. Per
  `perf-final-report.md` §3 item #7, the SoA split **must keep the full
  `genomes: Vec<Genome>` for cold paths** including inspection,
  mutation, speciation, save. The 7 SoA mirror arrays are **additive
  only**. Therefore Perf-5 does **not** change the wasm-bindgen surface
  or the JSON shape consumed by UI-Inspector. **No dependency** — but
  see §6 risk R1.
- **UI-Perf vs `profile_report_json`.** `web/src/perf.ts` (TS-side) and
  `WorldHandle::profile_report_json` (Rust) both already exist and emit
  the JSON contract pinned in `docs/plans/perf-timing.md §D5`. UI-Perf
  inherits the renderer code currently in `web/src/rail/stats.ts:101–270`
  *unchanged in shape*; it only re-hosts the HTML/CSS into a
  free-standing widget. **No dependency on `profile_report_json` format.**
- **Perf-2/3 vs SaveV1 / snapshot_hash.** `SaveV1`
  (`src/save.rs:23–62`) and `snapshot_hash::snapshot_hash`
  (`src/snapshot_hash.rs:20–80`) both list their inputs **explicitly,
  field-by-field**. Neither references `fx`, `fy`, the
  `eat_and_scavenge` scratch Vecs, nor `SpatialGrid` cursors. New
  pooled fields on `World` are automatically excluded from the hash so
  long as they are NOT added to either struct/function above. The
  fields on `World` use `#[serde(skip)]` only via the `SaveV1`
  field-by-field omission pattern (mirrors how `vision`,
  `cell_to_carrion`, `pending_extinction_check`, and `profile`
  already get omitted today — see `src/world.rs:91–101` for the
  pattern). **No serde annotation needed on `World` itself**; just do
  not add the new fields to `SaveV1`. Acceptance test
  `save_load_step_preserves_determinism`
  (`tests/acceptance.rs:113–148`) catches drift.
- **Perf-4 vs other perf pieces.** Perf-4 has no hard code dependency
  on Perf-1/2/3/5 — the threads path
  (`src/world.rs:351–424`) is its own `#[cfg(feature="threads")]`
  block. Because the default-build golden is **unchanged** (see §7),
  Perf-4 is no longer load-bearing for bisectability; it may land
  anywhere in the perf sequence. **Recommendation: still land Perf-4
  last in the perf block** so Perf-5 (the largest single-commit diff)
  serves as the natural mid-PR review checkpoint.
- **UI pieces vs each other.** All three rebuild the visible layout.
  They are HTML/CSS/TS only and touch disjoint widgets, but the rail
  `<aside id="right-rail">` (`web/index.html:23`) currently hosts all
  three of (charts, inspector dl, profiler table). The cross-reviewer
  must enforce: exactly one UI piece owns each former-rail child (see
  §6 R2). Suggested ownership: UI-Stats owns `#chart-pop` +
  `#chart-species` and the `maybeSampleStats` poll loop; UI-Perf owns
  the `#profiler-*` IDs and the entire `installProfilerPanel` block
  from `web/src/rail/stats.ts:101–270`; UI-Inspector owns the
  `#ins-*` dl + the inspector tab logic from
  `web/src/rail/inspector.ts`.

---

## 4. Commit ordering

One commit per piece. All eight pieces are golden-safe for the
default build. Perf-4 introduces a **new** threaded golden file plus
a `#[cfg(feature="threads")]` acceptance test (see §7); the existing
sequential golden is untouched. UI pieces are independent and may
interleave anywhere; for review clarity, batch them after the perf
pieces so the perf PR series stays focused.

Recommended order:

1. `perf(sim): pre-compute sector sin/cos per creature` *(perf-1)*
2. `perf(sim): pool per-tick scratch vecs on World` *(perf-2)*
3. `perf(sim): pre-allocate SpatialGrid cursor buffer` *(perf-3)*
4. `perf(layout): genome hot-field SoA split (7 scalars, mirror)` *(perf-5)*
5. `perf(threads): wire initThreadPool + parallel vision` *(perf-4)*
   — enables the `threads` feature end-to-end (JS `initThreadPool` +
   parallel vision), **adds** `tests/golden_snapshot_t10000_threaded.txt`
   and a new `#[cfg(feature="threads")]` acceptance test, and appends
   a DECISIONS.md entry explaining the dual-golden design. The default
   sequential golden is unchanged. See §7.
6. `feat(ui): top-left stats widget` *(ui-stats)*
7. `feat(ui): top-right inspector pop-up` *(ui-inspector)*
8. `feat(ui): bottom-left perf widget` *(ui-perf)*

Interleaving is allowed. Commits 1–4 form a coherent "perf v1, safe
subset" PR; commit 5 is its own PR ("threads + new threaded golden");
commits 6–8 are a "ui re-shape" PR. The UI PR may land before or
after the perf PRs without any code conflict — they touch disjoint
files (with one HTML edit each).

Each commit must independently pass:
- `cargo test --release --test acceptance`
- `cargo test --lib`
- `cargo fmt --check`
- `cargo clippy --all-targets -- -D warnings`
- `pnpm --filter ./web build`

Magic numbers go in `src/constants.rs` with a v5/v6 § citation or a
new `DECISIONS.md` line under a per-piece section.

---

## 5. Per-piece briefing pointers

For each piece: the spec/research the planner reads first, the files
they will touch, the tests that must stay green, and (UI pieces) the
DOM IDs that are stable contracts.

### perf-1 — sector sin/cos cache

- **Spec/research focus.** `docs/research/perf-final-report.md` §3
  item #1 (the lead win, 17–35% wall-time). `docs/research/perf-sim-hotpath.md` §1a.
- **Files touched.** `src/creature.rs` (add `eye_trig: Vec<[(f32,f32); EYE_SLOTS]>`
  parallel array, init on `push`, swap_remove on `remove_indices`,
  recompute helper `recompute_eye_trig(i, &Genome)`); `src/genome.rs`
  (call recompute in `mutate_in_place` if `eye_offsets` or `eye_count`
  mutated — author's call whether to invalidate inside genome or in
  caller); `src/vision.rs::VisionPass::fill_one` (read cached
  `(dx, dy)` instead of `theta_ray.cos()/sin()`); `src/world.rs` —
  call `recompute_eye_trig` in `handle_births` after the child is
  pushed.
- **Tests that must stay green.** All in `src/vision.rs::tests`,
  `cargo test --lib`, both acceptance tests (golden + profile).
- **Constants.** `EYE_SLOTS = 24` already in `src/constants.rs:102`.
  No new constants.

### perf-2 — scratch Vec pooling

- **Spec/research focus.** `perf-final-report.md` §3 items #2 and #4
  (bundled here per the §6 commit-2 shape: 8 Vecs). `perf-sim-hotpath.md`
  §2a, §2c, §2d.
- **Files touched.** `src/world.rs`. Nine per-tick Vecs to promote
  into `World` fields, explicitly enumerated (orchestrator-confirmed
  scope):
  - `fx: Vec<f32>` — movement/repulsion accumulator
    (`src/world.rs:480`).
  - `fy: Vec<f32>` — movement/repulsion accumulator
    (`src/world.rs:481`).
  - `neighbors: Vec<usize>` — repulsion neighbor scratch inside the
    `grid.for_each_in_radius` closure (`src/world.rs:487`). The
    implementer MUST handle the split-borrow dance: typical pattern
    is `let mut buf = std::mem::take(&mut self.scratch_neighbors);
    /* borrow self in the closure, push into buf */;
    self.scratch_neighbors = buf;`. This is non-negotiable per the
    orchestrator — it's the implementer's job.
  - The six `eat_and_scavenge` Vecs at `src/world.rs:611–616`:
    `damage`, `gain`, `cooldown_set`, `attempted_eat`,
    `attempted_scavenge`, `got_a_bite`. (Confirm exact names by
    reading `src/world.rs` — the planner should verify before
    writing field names.)
  Add a `clear()` at the top of each consumer site. Field naming
  convention: `scratch_fx`, `scratch_fy`, `scratch_neighbors`,
  `scratch_damage`, etc., grouped under a comment block near the
  other transient fields (`vision`, `cell_to_carrion`).
- **Tests that must stay green.** `world.rs::tests::*`, both
  acceptance tests.
- **Save/hash exclusion.** Do **not** add the new fields to
  `src/save.rs::SaveV1` or `src/snapshot_hash.rs::snapshot_hash`.
  Mirror the pattern used today for `vision` / `cell_to_carrion` /
  `pending_extinction_check` / `profile`.

### perf-3 — SpatialGrid cursor buffer

- **Spec/research focus.** `perf-final-report.md` §3 item #3.
  `perf-sim-hotpath.md` §2f.
- **Files touched.** `src/grid.rs`. Add `cursors: Vec<u32>` field
  on `SpatialGrid`; in `rebuild` (`src/grid.rs:37–58`), replace
  `let mut cursors = self.starts.clone()` (line 51) with
  `self.cursors.clear(); self.cursors.extend_from_slice(&self.starts);`
  (or `resize` + `copy_from_slice` if longer). Saves 3 allocations
  per tick (rebuild is called 3× per `step()`: line 188, line 478,
  line 558 of `world.rs`).
- **Tests that must stay green.** `grid.rs::tests`, both acceptance.

### perf-4 — threads end-to-end

- **Spec/research focus.** `perf-final-report.md` §3 items #5+#6
  (bundled). §5b (determinism budget — orchestrator-resolved via the
  dual-golden strategy: default sequential golden stays valid; a new
  threaded golden is added). `docs/research/perf-parallelism.md` §1, §2a.
- **Files touched.**
  - `web/src/main.ts`: `await initThreadPool(navigator.hardwareConcurrency)`
    before `new WorldHandle(...)`. SAB feature-detect fallback (if
    `crossOriginIsolated === false` OR `typeof SharedArrayBuffer === "undefined"`,
    skip the call — wasm still works sequentially because the
    `cfg(feature="threads")` branch becomes a sequential fallback when
    the rayon thread pool has 0 worker threads. Confirm rayon behavior
    OR run a smoke test before relying on the fallback).
  - `web/wasm/...` build: now consumes the `threads` feature — pin
    the build command in `web/package.json` or a `Makefile`-equivalent
    so CI builds the threaded artifact.
  - `src/vision.rs::VisionPass::run` (`src/vision.rs:53–58`): convert
    the sequential `for i in 0..n` to `par_iter_mut` over fixed
    N_CHUNKS=8 chunks of `out` (mirror v6 §J pattern already used in
    `nn_forward_all_chunks`). Vision is read-only per-creature aside
    from `out[i]`, so the chunked disjoint-slice approach is sound.
    Behind `#[cfg(feature="threads")]`; sequential path stays
    untouched.
  - `Cargo.toml`: nothing — `threads` feature is already declared
    (line 38).
  - `tests/golden_snapshot_t10000.txt`: **unchanged.** Default-build
    acceptance stays green against the existing pinned hash.
  - `tests/golden_snapshot_t10000_threaded.txt`: **new file.**
    Bootstrap via the dual-golden protocol in §7. Pinned thereafter.
  - `tests/acceptance.rs`: add a second test gated on
    `#[cfg(feature = "threads")]` that runs the same 10k-tick
    scenario with rayon enabled and asserts against the new
    threaded golden.
  - CI workflow: add a step that runs `cargo test --release
    --features threads --test acceptance` (in addition to the
    unconditional default-build acceptance) so the threaded golden
    is exercised automatically. Skip if CI integration is not
    straightforward — the test still runs locally on demand.
  - `DECISIONS.md`: append entry under a `## v1.1 perf` section
    explaining the dual-golden design (why two files, what each
    pins, when each gets regenerated).
- **Tests that must stay green.** Default build: `cargo test --lib`,
  `cargo test --release --test acceptance` (sequential golden
  unchanged), `pnpm build`. Threaded build: `cargo test --release
  --features threads --test acceptance` against the new threaded
  golden.
- **Determinism contract preserved.** N_CHUNKS=8 fixed-chunk pattern
  per v6 §J + per-chunk sub-RNG (already in place for NN forward).
  Vision is side-effect-free per-creature (no RNG draw, no shared
  write) so par_iter_mut over disjoint `out` slices is bit-identical
  to the sequential path. The threaded golden differs from the
  sequential golden because of the **NN-forward** path's sub-RNG
  ordering, per `perf-final-report.md` §5b first bullet — the
  threaded codepath is deterministic against itself, just not
  bit-identical to the sequential codepath.
- **Build infra.** COOP/COEP headers ship in `web/_headers` per
  `BUILD-REPORT.md`. wasm-bindgen-rayon dep is declared (Cargo.toml
  line 40). The only missing link is the `--features threads` flag
  on the wasm-pack build command.

### perf-5 — genome SoA split (7 fields, mirror)

- **Spec/research focus.** `perf-final-report.md` §3 item #7
  (the natural review checkpoint per §6 commit 5). `docs/research/perf-layout.md`
  §1 (hot-field set: size, photosynth_efficiency, eat_efficiency,
  scavenge_efficiency, move_speed, vision_range, eye_count).
- **Files touched.** `src/creature.rs` (add seven `Vec<f32>` /
  `Vec<u8>` parallel arrays to `CreatureSoA`; update `push`,
  `remove_indices`, `with_capacity` — exactly mirror the existing
  pattern). `src/world.rs` (rewrite the six hot read sites identified
  in `perf-final-report.md` §3 item #7: `photosynth_two_pass`,
  `energy_bookkeeping`, `apply_movement_and_repulsion`,
  `build_nn_input`, `count_carrion_overlap`/`compute_is_at_wall`,
  `eat_and_scavenge` — read from the new arrays instead of
  `creatures.genomes[i].field`). `src/vision.rs::VisionPass::fill_one`
  (read `eye_count_hot` and `vision_range_hot` from new arrays).
  Add a `sync_hot_fields(i, &Genome)` helper on `CreatureSoA` called
  after `push` and after any in-place mutation in `mutate_in_place`
  (or in `handle_births` right after `child_genome` is finalized but
  before `creatures.push`).
- **Critical rule.** `genomes: Vec<Genome>` stays. Cold paths
  (mutation, speciation, `creature_inspect_json`, `SaveV1` round-trip,
  `species_distance`, `snapshot_hash::hash_genome`) continue to read
  the AoS Genome. The mirror is **redundant storage** — same bytes,
  different layout, only consulted on hot paths. The snapshot hash is
  unchanged (it hashes the AoS Genome). Save round-trip is unchanged
  (it serializes the AoS Genome and on load calls `sync_hot_fields`).
- **Tests that must stay green.** Every test in `cargo test --lib`,
  plus both acceptance tests. The
  `save_load_step_preserves_determinism` test is the single best
  catch for "implementer forgot to sync the SoA mirror on load".

### ui-stats — top-left Stats widget

- **Spec/research focus.** v5 §11 (Stats panel intent — pop + species
  charts), v6 §A (DOM-only, no framework). Not in any perf doc.
- **Files touched.** `web/index.html` (add `<div id="stats-box">`
  containing the two existing canvases, positioned top-left). Move
  `<canvas id="chart-pop">` and `<canvas id="chart-species">` out of
  `#rail-stats` into the new box. `web/src/styles.css` (position
  fixed top-left, similar pattern to `#top-bar` but anchored under
  it; respect viewport-fit safe-area). `web/src/rail/stats.ts` —
  rename to `web/src/widgets/stats.ts` (or keep file, just remove
  the profiler block). The existing `maybeSampleStats` poll function
  stays; just decouple from the rail container. `web/src/rail/index.ts`
  loses its `maybeSampleStats(world)` call only if Stats is no longer
  "polled through pollRail" — author's call: keep polling from
  `pollRail` (which still runs every frame from `main.ts`) is the
  least-churn option.
- **Stable DOM contracts that MUST be preserved.** `#chart-pop`,
  `#chart-species` IDs (consumed by `getCanvases()`). `#top-bar`,
  `#status`, `#seed-display`, `#seed-value`, `#seed-copy-btn`,
  `#save-indicator`, `#toast-stack` must continue to render and work
  exactly as today. `#aquarium` canvas must still fill the viewport
  underneath the new boxes.
- **Z-order.** Above canvas (z-index ≥ 5, matching `#right-rail`),
  but boxes must not capture pointer events on transparent regions —
  add `pointer-events: none` on the outer container and
  `pointer-events: auto` on the chart elements (mirror the
  `#top-bar` pattern at `styles.css:42–48`).
- **Tests.** No Rust tests touched. `pnpm build` must stay clean.

### ui-inspector — top-right Inspector pop-up

- **Spec/research focus.** v5 §11 (Inspector intent). E.24 in
  `DECISIONS.md` for the existing click semantics. Not in any perf
  doc.
- **Files touched.** `web/index.html` (move the `<dl>` block from
  inside `<section id="rail-inspector">` into a new
  `<div id="inspector-box" hidden>`). `web/src/styles.css` (position
  fixed top-right, hidden by default, slide-in transition optional
  but **out of scope**). `web/src/rail/inspector.ts` — change the
  `openInspector`/`clearSelection` functions to show/hide the new box
  via `hidden` attribute or `.is-open` class rather than via tab
  switching. `installCanvasClickHandler` stays — its hit-test logic
  is unchanged. The `ensureInspectorTab` / `rail.switchTab("inspector")`
  / `rail.switchTab("events")` calls become no-ops or get removed
  (inspector no longer lives in the rail).
- **Stable DOM contracts that MUST be preserved.** Every `#ins-*` ID
  (15 of them: `#ins-species` through `#ins-pigment`) — these are
  written by `renderInspector` at `inspector.ts:34–54`. The
  `CreatureInspectJson` TS interface stays; the wasm
  `creature_inspect_json` JSON shape stays. Click handler IDs
  (`#aquarium`) stay.
- **Right-rail disposition.** Orchestrator-resolved (see §8.1):
  leave `<aside id="right-rail">` in HTML as an empty shell hidden
  via `display: none`, matching the pattern already used for
  `#rail-events` (`index.html:31`). Preserve `installRail` /
  `pollRail` plumbing in `web/src/rail/index.ts` and the
  highlight-management code in `rail/highlight.ts`. UI-Inspector
  (and the other UI pieces) must evict their former-rail children
  into their own boxes WITHOUT deleting the rail container or its
  sibling modules. Future v1.1 re-enablement of the rail stays
  cheap.
- **Tests.** No Rust tests touched. `pnpm build` must stay clean.

### ui-perf — bottom-left Perf widget

- **Spec/research focus.** `docs/plans/perf-timing.md` §D5 (JSON
  contract — DO NOT change), §"web/src/rail/stats.ts edits"
  (the inherited renderer). The code-review block at the bottom of
  that doc notes the table-too-thin bug; this widget fixes it.
- **Files touched.** `web/index.html` (extract the
  `<div id="profiler-panel">` block at lines 35–50 into a new
  free-standing `<div id="perf-box">` positioned bottom-left).
  `web/src/styles.css` (replace `.profiler-table` width-constrained
  rules at lines 312+ with `table-layout: auto; min-width: 480px;
  white-space: nowrap;` so columns size to content and don't
  truncate; widen the box and let it scroll horizontally if needed).
  `web/src/rail/stats.ts` — extract the entire profiler-panel block
  (`installProfilerPanel`, `pollAndRender`, `renderProfilerTable`,
  etc. — `web/src/rail/stats.ts:101–270`) into a new
  `web/src/widgets/perf.ts`. `web/src/main.ts` — update the import
  (`import { installProfilerPanel } from "./rail/stats"` →
  `from "./widgets/perf"`).
- **Stable DOM contracts.** `#profiler-enable` (checkbox),
  `#profiler-table`, `#profiler-tbody`, `#profiler-stabilizing` —
  all consumed by the renderer code being moved. Keep IDs intact.
- **Wasm API contract.** `WorldHandle.profile_enable(bool)` and
  `WorldHandle.profile_report_json()` stay exactly as today
  (`src/wasm_api.rs:336–351`). The TS-side `web/src/perf.ts` API
  (`attachProfiler`, `setProfilerEnabled`, `span`, `timed`,
  `reportJson`, `isProfilerEnabled`) stays exactly as today.
- **Z-order + pointer-events.** Same pattern as ui-stats: outer
  container `pointer-events: none`, interactive children
  `pointer-events: auto` (the checkbox in particular must remain
  clickable).
- **Tests.** No Rust tests touched. `pnpm build` must stay clean.

---

## 6. Cross-piece risks (for the cross-reviewer)

The reviewer compares all eight per-piece plans; these are the
collisions to grep for.

**R1. Perf-5 SoA mirror vs UI-Inspector / `creature_inspect_json`.**
If the perf-5 planner proposes **replacing** `genomes: Vec<Genome>`
rather than **mirroring** it, every cold-path reader breaks: the
inspector JSON, the snapshot hash (which calls
`hash_genome(&w.creatures.genomes[i])` at `snapshot_hash.rs:38`),
`species_distance`, `Brain::child_from`, `Genome::mutate_in_place`,
the founder anchors, the HoF clone sites, and `SaveV1`. Required
contract: the AoS `Vec<Genome>` STAYS; the seven hot scalars are
ADDED as parallel arrays. Reject any plan that proposes "remove
`genomes` after migration".

**R2. UI-Stats vs UI-Perf inheritance of the profiler-table renderer.**
The current `web/src/rail/stats.ts` (lines 1–99 charts; lines 101–270
profiler) bundles two unrelated widgets. Exactly ONE of the two UI
plans inherits each block. Mandated split: **UI-Stats** takes
`maybeSampleStats` + `drawCharts` + the two `#chart-*` canvas IDs;
**UI-Perf** takes everything from `// ─── Profiler panel ───`
(line 101) onward including `installProfilerPanel`, `startPolling`,
`pollAndRender`, `renderProfilerTable`, `escHtml`, and the
`#profiler-*` DOM IDs. Reject any plan where both widgets claim the
same code.

**R3. Right-rail teardown vs `pollRail`.** UI-Stats and UI-Inspector
both want to evict their content from the rail. `pollRail`
(`web/src/rail/index.ts:92–123`) currently calls `maybeSampleStats`,
`refreshInspector`, `tickToasts`, `pruneHighlights`, and a 1Hz
species poll. If both UI planners independently strip from the rail,
the orchestrator needs the answer to open question §8 to know
whether `pollRail` keeps existing (with reduced responsibilities)
or is dissolved. Reviewer's check: there must be exactly one home
for each of the 5 `pollRail` responsibilities after the dust
settles.

**R4. Perf-2 field naming collision with profiler.** `World` already
has 9+ fields with non-trivial names; perf-2 adds 9 more
(`scratch_fx`, `scratch_fy`, six eat/scavenge bools, `neighbors_buf`).
Reviewer's check: no name collision with `profile`, `vision`,
`cell_to_carrion`, `pending_extinction_check`, `events`,
`sliders`, `peak_*`. Suggest a leading `scratch_` prefix for all
nine to make them visually grouped and easy to spot during save/hash
review.

**R5. Perf-4 build command divergence.** wasm-pack today builds
**without** `--features threads`. The Perf-4 planner must update
whatever script invokes wasm-pack to include the flag so the browser
runtime gets the threaded artifact. Reviewer's check: every wasm-pack
invocation in the repo (grep `wasm-pack`) is updated.
**Orchestrator-resolved (dual-golden):** the default `cargo test
--release --test acceptance` continues to run in **sequential** mode
against the existing pinned `tests/golden_snapshot_t10000.txt` (which
stays valid — no regen). A second test gated on
`#[cfg(feature = "threads")]` runs the same 10k scenario with rayon
enabled and asserts against the new
`tests/golden_snapshot_t10000_threaded.txt`. CI runs the sequential
acceptance unconditionally; the threaded acceptance runs whenever
`--features threads` is passed (add a CI step that does this if
straightforward). This means CI cannot ship a green default build
while silently breaking the threaded path, and bisecting either
codepath is unambiguous.

**R6. Perf-1 trig cache invalidation completeness.** The cache is
"recompute on birth/mutation". The two write sites are
`handle_births` (`src/world.rs:878–957`) and `mutate_in_place`
(in `src/genome.rs`). The founder is `push`ed in `World::new`
(line 119) — that's a third write site that must also call
`recompute_eye_trig`. The `from_save_v1` path (`world.rs:999`)
restores creatures via `creatures.push(...)` — that's a fourth site.
Reviewer's check: all four sites recompute the trig cache, OR the
cache is computed inside `CreatureSoA::push` and never anywhere else
(safer, one location). Recommend the latter; the perf-1 planner
should choose explicitly.

**R7. Perf-5 hot-field sync completeness.** Same shape as R6: every
write site for `genomes[i].field` for any of the 7 hot fields must
also write the SoA mirror. Sites: `push` (init), `handle_births`
(birth + mutation), `from_save_v1` (restore). `mutate_in_place`
mutates in place but only inside `handle_births`, so the call in
`handle_births` covers it provided the sync is called AFTER mutation
and BEFORE `creatures.push`. Reviewer's check: there is no `let g =
&mut self.creatures.genomes[i]; g.size = …` anywhere outside the
acknowledged sites. Recommend the perf-5 planner add a debug-only
sanity check that asserts mirror == AoS at end of `World::step` in
`cfg(debug_assertions)`.

**R8. UI z-order vs `#right-rail`.** All three UI boxes set
`position: fixed`; the existing `#right-rail` is also `position:
fixed` with `z-index: 5`. Orchestrator-resolved (§8.1): `#right-rail`
is hidden via `display: none` and stays out of the visual stack, so
no overlap with the new UI boxes is possible. The z-order ladder
within the three UI boxes themselves still needs to be disjoint
(top-left Stats, top-right Inspector, bottom-left Perf — no
overlap by design).

---

## 7. Threads dual-golden protocol (Perf-4 only)

Perf-4 does **not** regenerate the existing
`tests/golden_snapshot_t10000.txt`. The default sequential build
stays bit-identical. Instead, Perf-4 **bootstraps a new threaded
golden file** that pins the (different, but still deterministic)
hash produced by the rayon codepath. Protocol:

1. The implementer makes all code changes (JS `initThreadPool` +
   parallel vision + any wasm-pack build-command updates so the
   browser artifact gets `--features threads`).
2. Confirm `cargo test --lib` is green (default build, no features).
3. Confirm `cargo test --release --test acceptance` is green
   (default build, sequential — the existing pinned
   `tests/golden_snapshot_t10000.txt` must still match. If it
   doesn't, the implementer accidentally changed the sequential
   codepath; fix that before continuing — do NOT regen.)
4. Add the new threaded acceptance test in `tests/acceptance.rs`,
   gated on `#[cfg(feature = "threads")]`. It mirrors the 10k-tick
   scenario, asserts against `tests/golden_snapshot_t10000_threaded.txt`,
   and supports a write-the-golden bootstrap path via an env var.
   Suggested name: `EVOSIM_WRITE_GOLDEN_THREADED=1` (implementer
   picks a clean name; document it in the test's doc-comment and in
   the DECISIONS.md entry).
5. Bootstrap the new golden: run
   `EVOSIM_WRITE_GOLDEN_THREADED=1 cargo test --release
   --features threads --test acceptance`. This **writes**
   `tests/golden_snapshot_t10000_threaded.txt`.
6. Re-run `cargo test --release --features threads --test acceptance`
   **without** the env var; the threaded test must pass against the
   new pinned file.
7. Re-run `cargo test --release --test acceptance` (default build,
   no features) one more time to prove the sequential golden is
   still pinned and green.
8. Stage BOTH the new test code and the new golden file in the SAME
   commit as the threads enablement code. Do NOT split.
9. Append a `DECISIONS.md` entry under a `## v1.1 perf` section
   stating: dual-golden design rationale (default build unchanged,
   threaded codepath has its own pinned hash because sub-RNG ordering
   differs per `perf-final-report.md` §5b first bullet); the
   bootstrap env var name; the regen rule (only regen the threaded
   golden when the threaded codepath itself intentionally changes —
   never as a "fix" for accidental drift); a pointer to this plan.
10. CI: if straightforward, add a step that runs `cargo test
    --release --features threads --test acceptance` so the threaded
    golden is exercised on every push. If CI integration is gnarly,
    document the local manual-run command in the DECISIONS.md entry
    and leave CI for a follow-up.

If either test fails with a hash mismatch after step 6 or 7, the
implementer made a mistake — DO NOT re-bootstrap the golden to paper
over it. Diagnose first.

---

## 8. Resolved decisions

The three questions the master planner originally raised have been
answered by the orchestrator. Recorded here for downstream planners.

1. **Right-rail disposition after UI eviction — RESOLVED: keep
   hidden.** Leave `<aside id="right-rail">` in `web/index.html` as
   an empty shell, hidden via `display: none` (CSS or inline).
   Preserve the `installRail` / `pollRail` plumbing in
   `web/src/rail/index.ts` and the highlight module
   (`web/src/rail/highlight.ts`). Rationale: minimizes blast radius
   and matches the existing Events-disable pattern from
   DECISIONS.md ("Events disable (v1.1 revisit)") — keep DOM nodes
   around for future re-enablement, just hide. The three UI widget
   plans (ui-stats, ui-inspector, ui-perf) MUST treat the rail as a
   deprecated container they **evict from**, not as a thing to
   delete. The rail's other modules (events, toast, highlight) stay
   in place.

2. **Perf-4 determinism strategy — RESOLVED: dual-golden.** Do NOT
   regenerate `tests/golden_snapshot_t10000.txt`. The default
   sequential acceptance test stays green against the existing
   pinned hash. ADD a second pinned golden file
   `tests/golden_snapshot_t10000_threaded.txt` and a second
   acceptance test gated on `#[cfg(feature = "threads")]` that runs
   the same 10k-tick scenario with rayon enabled. CI runs the
   sequential acceptance unconditionally and the threaded
   acceptance whenever `--features threads` is passed (add a CI
   step if straightforward). Perf-4 is therefore **golden-safe for
   the default build** — it introduces a new golden, not a regen.
   See §7 for the bootstrap protocol and §4 for the updated commit
   shape.

3. **Perf-2 `neighbors` buffer — RESOLVED: include.** The
   `neighbors: Vec<usize>` repulsion buffer is in scope for Perf-2.
   The split-borrow dance around `grid.for_each_in_radius` is
   required — that's the implementer's job to handle (typical
   pattern: pull the field out via `std::mem::take`, borrow `self`
   for the closure, swap back). See the updated Perf-2 piece
   briefing in §5 for the full enumerated field list (nine Vecs:
   `fx`, `fy`, `neighbors`, and the six `eat_and_scavenge`
   scratch Vecs).

---

## 9. Cross-review findings

After all 8 detailed plans landed, a cross-piece review
(`docs/plans/perf+ui-cross-review.md`) compared them for hidden
collisions and integration risks no per-piece reviewer would catch.
Verdict: **no commit-ordering changes required.** Five must-fix items
for the per-piece implementers, summarized here so the orchestrator
sees them in one place:

- **M1.** perf-5 must include an explicit "merge against perf-1" note
  for `CreatureSoA::push` / `with_capacity` / `remove_indices`.
  perf-1 adds `eye_trig` field + `recompute_eye_trig_at` + a
  `swap_remove_chunk` call to the same three functions. The perf-5
  implementer must insert `push_hot_mirrors(&genome)` BEFORE
  `self.genomes.push(genome)` and leave perf-1's lines after.
- **M2.** perf-5 must include an explicit "merge against perf-2" note
  for `apply_movement_and_repulsion` and `eat_and_scavenge`. perf-2
  rewrites `fx`/`fy`/`neighbors`/eat-scratch accessors at slightly
  different lines than perf-5's `g.size`/`g.eat_eff` rewrites. The
  `for j in neighbors` loop header becomes `for &j in &neighbors`
  after perf-2's `mem::take` recipe — perf-5 must not regress it.
- **M3.** perf-5 §"Sequencing" must add `cargo clippy --all-targets
  --features threads -- -D warnings` between steps 4 and 5. perf-5
  Site G edits the threaded NN block at `world.rs:374`; default-build
  clippy won't validate it. This check works even before perf-4 ships.
- **M4 / M5.** `.overlay-widget` `pointer-events` policy is
  inconsistent between ui-stats (`none` on container, `auto` on
  children — for canvas-pan-through) and ui-inspector (which is opaque
  and needs `auto` on the container itself). Resolution: make
  `.overlay-widget` default to `pointer-events: auto`; ui-stats and
  ui-perf each explicitly set `pointer-events: none` on their own
  `#stats-box` / `#perf-box` (with child `auto` opt-in); ui-inspector
  inherits the `auto` default. Inverts ui-stats §3a's current rule.

Two should-fix items: **S1** asymmetric DECISIONS.md coverage (only
perf-4 and ui-perf write entries; either harmonize or drop ui-perf's
plan to write them); **S2** perf-4 §R3's Fix A choice should be
pinned in the plan rather than deferred to "implementer decides
during step 5" — pre-decide Fix A.

Verified-clean (no action needed): perf-5/UI-Inspector compatibility
(R1 contract honored); perf-4 wasm-bindgen export surface (only
`init_thread_pool` added); profiler JSON contract (untouched by all
five perf pieces and ui-perf); the existing sequential golden
`tests/golden_snapshot_t10000.txt` (zero plans touch it); single
ownership of `#right-rail { display: none }` (ui-stats only); DOM ID
union (none of the 30+ preserved IDs are dropped); polling-cadence
independence (stats-via-pollRail, inspector-via-pollRail-when-open,
profiler-via-setInterval-when-checked); CSS class definition uniqueness
(`.overlay-widget` defined once by ui-stats, consumed by the other
two).

See `docs/plans/perf+ui-cross-review.md` for the full per-pair
verdict table, the file-by-file integration timeline, and the
risk-tier summary.

---

*End of master plan.*
