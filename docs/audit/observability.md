# Observability Audit — profiler + events + perf surface

Scope: `src/profiler.rs`, `src/events.rs`, `src/wasm_api.rs`,
`web/src/perf.ts`, `web/src/rail/stats.ts` (profiler panel),
`web/src/main.ts` (frame loop). Cross-checked against
`docs/plans/perf-timing.md` and `docs/plans/ui-perf-box.md`.

Bottom line: the hierarchical span tree is solid and the JSON contract
is well-defined, but the observable surface is **wide-only** (12 top
spans) and **shallow** (no per-stage counters, no histograms, no
regression detection, no event firehose, no thread/parallel attribution).
The default ship is "Stats charts + Perf table" — that is *less*
observable than what the engine can already emit cheaply.

---

## 1. Profiler overhead in the hot path — OK, with two latent footguns

`src/profiler.rs:106-138` is the hot path. Disabled cost is one
`AtomicBool::load(Relaxed)` plus a predicted branch per `span!` site (12
sites/tick = ~24 cycles/tick); negligible. Enabled cost is bounded by
the test at lines 817-870 to < 5 µs per push/drop pair.

Latent issues:

- **`src/profiler.rs:235` allocates a `String` per never-seen `(parent,
  name)` pair via `key = (parent, name.to_string())`.** Cached after
  first call, so amortized to zero, but this is also the path for
  TS-side `record_external("renderWorld.drawCreatures", ...)` — every
  call splits on '.' and re-allocates the lookup key per segment. Fine
  today (per-frame, fixed names) but if any TS span ever interpolates a
  dynamic name (e.g. species id), it leaks one HashMap entry per unique
  string forever.  Mitigation: assert `&'static`-ness at the macro
  boundary (already true for Rust via `$name:literal`), and document
  the TS side cannot pass dynamic path components.

- **`src/profiler.rs:114-120` `set_enabled(true)` resets `epoch_ms` but
  does not call `clear_samples()`.** A second on→on toggle (race with
  the UI) would leave stale samples with timestamps past the new epoch;
  pruning never reaches them. The post-PR review note at
  `docs/plans/perf-timing.md:1261` already flags this. Fix is one line.

- **Profiler is `!Send`/`!Sync` but the threads path
  (`src/world.rs:373-446`) is hot.** No spans inside the rayon
  `par_iter`. Today this is by design (`docs/plans/perf-timing.md`
  R12), but it means the threaded NN-forward stage is observed as a
  single `nn_forward` total with no per-chunk attribution and no way
  to see thread imbalance. See §3.

---

## 2. Missing critical timing spans

`World::step` has 12 top-level spans (one per architecture tick step)
and zero sub-spans. The inner hot routines are black boxes:

| Sub-function (src/world.rs)                | Why a sub-span is wanted |
|---|---|
| `run_vision_pass` → `VisionPass::run` (`vision.rs:151` raycast)  | Vision is consistently top-3 cost. Today we cannot tell raycast vs cell-iteration vs `build_cell_to_carrion`. Add `vision_pass.cell_index_rebuild` + `vision_pass.raycast` (one span around the inner loop, NOT per creature — per-creature would explode the ring buffer per `docs/plans/perf-timing.md` "Out-of-scope"). |
| `nn_forward_all_chunks` (sequential)       | Add `nn_forward.input_pack`, `nn_forward.matmul`, `nn_forward.action_decode` — three sub-spans per chunk would cost ~30 µs over a 1500-creature tick and would give the user the first real signal on where to spend optimization budget. |
| `nn_forward_all_chunks` (threads)          | Currently invisible (see §1). Need per-chunk timing recorded *after* the parallel join (chunk index encoded in the span name `nn_forward.chunk_0` … `nn_forward.chunk_3`). |
| `apply_movement_and_repulsion`             | Internally rebuilds the grid (the `grid_rebuild_2` deferred by `docs/plans/perf-timing.md` D5). Worth splitting now: `movement.integrate` + `movement.repulsion` + `movement.grid_rebuild_2`. |
| `eat_and_scavenge`                         | ~700 lines (`src/world.rs:638-761`). Almost certainly hides cost. Split into `eat.gather_neighbors`, `eat.resolve_bites`, `eat.scavenge`. |
| `photosynth_two_pass`                      | Two passes; we only see the sum. `photosynth.pass1` + `photosynth.pass2`. |
| `handle_births`                            | Calls `species_distance` per birth — variable cost with mutation rate. Worth `births.mutate`, `births.speciation_check`. |
| `from_save_v1` / `to_save_v1`              | Save round-trip is unmeasured. The plan says "1–5 ms" at v1 sizes but there is no span. Add `save.serialize` and `save.deserialize` at the wasm_api boundary. |

Frame side (`web/src/main.ts:271-309`) has `step_n`, `maybeAutosave`,
`pollRail`, `renderWorld`. Missing:

- **No span around `creature_ids_buffer()`** (line 295). It walks the
  full id Vec every frame; should be timed.
- **No spans inside `renderWorld`** despite `docs/plans/perf-timing.md`
  §"Files & function signatures" promising `drawSunMap` / `drawCarrion`
  / `drawCreatures` sub-spans. Implementation never landed —
  `grep -n "span\|timed" web/src/render.ts` is empty.
- **No span around `pollRail`'s species poll** (the 1 Hz
  `species_list_json()` JSON parse can be tens of KB at high species
  counts).

---

## 3. Observability gaps — what you cannot see today

Things the engine knows but does not surface:

1. **Tick rate / TPS.** Nowhere computed. UI prints `tick N` in the
   status bar but does not show `ticks/sec`. The profiler shows
   `tick.call_count` over a window, but the user has to divide
   manually. Trivial counter: emit `tps_1s` and `tps_60s` from the
   profiler ring (`call_count / effective_window_ms`).
2. **Frame rate / dropped frames.** RAF callback exists but no
   `frame.dt_ms` histogram, no count of frames where `delta > 33ms`
   (jank). One TS counter, free.
3. **Population dynamics rate.** `births_this_tick`, `deaths_this_tick`,
   `speciations_this_tick`, `extinctions_this_tick`. Currently only
   visible as `population` delta or by polling the event log (which is
   disabled — see §4). Cheap to expose as four `u32` per tick.
4. **Vision-pass workload size.** Total raycasts/tick is unmeasured.
   At 1500 creatures × variable eye_count, this is the actual cost
   driver. Expose `vision_rays_total: u64` (cumulative) and let the UI
   diff.
5. **Memory.** No `wasm.memory.buffer.byteLength` poll. The scratch
   pools and ring buffers can creep; nothing reports it.
6. **Carrion count, sun-cell saturation.** Both are simple
   instantaneous counters. The renderer reads `carrion_buffer` so it
   knows the count, but it's never logged.
7. **Per-chunk thread imbalance.** See §2. With perf-4 threads landed,
   knowing whether chunk 0 finishes 3 ms before chunk 3 is the only way
   to know if the chunking is right.
8. **Save size.** `snapshot_json()` length is the autosave footprint;
   never logged.
9. **`live_species_count` history beyond chart.** Chart is a 500-sample
   ring (`web/src/rail/stats.ts:14`); no max/min/p95 surfaced anywhere
   else.
10. **Profiler self-stats.** `nodes.len()`, `child_index.len()`,
    memory used by the ring buffers — invisible. A user hitting
    `MAX_NODES=256` would silently see spans collapse into their
    parent (`src/profiler.rs:240-244`) with no warning.

---

## 4. Event firehose risks

`src/events.rs` is **already gated off** in default builds
(`events_enabled: false` at `src/world.rs:155`). All six event types
are constructed behind `if self.events_enabled` (8 call sites). The
rail-side polling is also dead (`web/src/rail/index.ts:69, 112`). So
today the event subsystem is dormant; the audit is a forward-look.

When re-enabled (post-v1.1), the risks are real:

- **`EventLog::all: Vec<Event>` is unbounded.** `src/events.rs:43` —
  grows for the entire run, saved verbatim into `SaveV1`
  (`src/save.rs:178`). At ~1 Speciation event per ~10 births × hours
  of sim, the `all` vec can reach 100k+ entries. Each event carries
  one or two heap-allocated `String` (species name). Allocation per
  event: ~2 × `String::from("Species-N")` ≈ 80 B + 32 B struct =
  ~112 B/event; 100k events = 11 MB extra in the save file.
  **Mitigations to plan:** (a) intern species names by id and store
  `species_id` instead of `String` in `EventKind`, then look up at
  display time; (b) cap `all` and roll into the ring like `recent`,
  losing replay-completeness but keeping the save bounded; (c) keep
  full history but compress (RLE PopulationMilestone, dedup species
  names).
- **Allocation per event push.** `src/events.rs:59-65` does `ev.clone()`
  + `Vec::push` + `VecDeque::push_back` — the clone deep-copies the
  inner `String`s. For a Speciation event with two names, that is two
  heap allocations per push. At a population-milestone storm (100 →
  500 → 1000 → 5000 → 10000) firing in a single tick, that's 10+
  pushes; fine. But during a mass-extinction cascade (Extinction per
  species died, hundreds in one tick), this becomes the dominant
  bookkeeping cost.
- **Ring cap is 200 (`src/events.rs:54`).** Hard-coded. No way to
  resize from the UI; no way for a user staring at "give me the last
  hour of events" to get them without reaching into `all`. Plan a
  configurable cap (constant in `constants.rs` at least).
- **No per-tick rate limit / no de-dup.** A future "creature attacked"
  event would fire 1000+ times per tick. The current design has no
  brake; the ring would burn through 200 entries instantly. Worth
  designing in **before** re-enabling.
- **`recent_events_json()` (`src/wasm_api.rs:187`) serializes the
  entire 200-entry deque on every call.** When the UI re-enables event
  polling, this will be a per-frame cost. The diff-via-monotone-counter
  pattern at `events_total_count` is correct but the *payload* still
  ships the full 200.  Add a `recent_events_since(seq: u32)` variant
  that returns only new entries.

---

## 5. Missing counters / histograms

The profiler stores `(timestamp_ms, dur_us)` — i.e. it already has the
raw data for histograms but throws away anything except sum and count.
Adding p50/p95/p99 to the JSON is a single pass over the per-node ring
on report (`src/profiler.rs:319-366` — `serialize_node` already walks
the deque to sum; sort-in-place + index would cost ~O(N log N) per
report at 1 Hz, negligible at ~4096 samples).

Concrete additions to the JSON node shape:

- `p50_us`, `p95_us`, `p99_us`, `max_us` (sort copy, index, no
  precision loss).
- `min_us` (cheap, useful for detecting Spectre-clamped zero spans).
- `last_us` (most-recent sample only; replaces "live spike" feel that
  rolling averages hide).

New first-class counters (separate API, not in the span tree):

- `WorldHandle::tick_rate_1s() -> f32` (derived from profiler `tick`
  node sample window, but easier for UI than re-parsing the JSON).
- `WorldHandle::counters_json()` → `{ births, deaths, speciations,
  extinctions, vision_rays, carrion, save_bytes_last }`.
- `WorldHandle::profiler_node_count() -> u16` to detect approaching
  `MAX_NODES`.

---

## 6. No perf regression detection

This is the biggest gap. The codebase has:

- A native `cargo test` overhead test
  (`src/profiler.rs:817-870`) that asserts < 5 µs per span pair —
  catches catastrophic regressions only.
- Acceptance hash tests
  (`tests/acceptance.rs::profile_does_not_change_hash`) — determinism,
  not performance.
- A 10k-tick golden snapshot — correctness, not performance.

Nothing detects "perf-4 threads slowed the sequential path by 20%"
or "perf-5 SoA split made eat_and_scavenge 2× slower". The user has
to eyeball the profiler table.

Recommended additions (separate plan, not part of this audit):

- **`tests/perf_budget.rs`**: in release mode, run 10k ticks at 1500
  creatures, assert `step_median_us < BUDGET` per stage. Budgets
  pinned per stage; CI fails on regression. Budgets re-pinned
  consciously when an optimization lands.
- **Tracked baseline file**: `tests/perf_baseline.json` with the
  median µs per top-level span. The test reads it, asserts ≤ 110%
  of baseline. PR-author re-pins on intentional changes.
- **Browser-side counter sample**: log `tick_ms_median` at run end to
  a hidden devtools-accessible `window.__perf_summary` for ad-hoc
  manual diffs.

---

## 7. UI surfaces that should be wired up

Reading `docs/plans/ui-perf-box.md` — the Perf box is the *only*
observability surface in v1.1. The Stats box owns charts (pop /
species, both at 1 Hz). Nothing surfaces:

| Surface | Where it should live |
|---|---|
| **Tick rate (TPS, last 1s + last 60s)** | Top of Perf box header, next to the profiler checkbox. One number. |
| **Frame rate / jank count** | Same row, right side. `60.0 fps · 3 jank/60s`. |
| **Effective sim speed multiplier vs realtime** | `step_n` total time / wall time. Already implicit from the profiler but should be a top-line readout. |
| **Stabilizing banner** | Already implemented (`web/src/rail/stats.ts:210-219`). Good. |
| **Profiler node-count usage** | Footer line: `nodes: 32/256` so the user sees if they're approaching MAX_NODES. |
| **Per-node `last_us` / `max_us` columns** | Adds two columns to the existing 6, hidden behind a "details" toggle. The Perf box is widening anyway (`docs/plans/ui-perf-box.md` D4 `min-width: 560px`). |
| **Save size** | Bottom of Perf box: `last save: 1.2 MB · 4.7s ago`. |
| **WASM memory** | Bottom of Perf box: `wasm: 48 MB`. |
| **Pop dynamics rates (births/s, deaths/s, speciations/min, extinctions/min)** | Either a 4-row mini-table in the Stats box or a row in the Perf box. The data is already in `events.all` even when the UI poll is dead — the wasm side just needs `WorldHandle::pop_rates_json()`. |
| **Per-chunk thread balance** | Sub-rows under `nn_forward` in the profiler tree (already supported by the tree shape — just need the spans, see §2). |
| **Vision rays/tick** | Sub-row under `vision_pass`. Same shape. |

---

## 8. Concrete actions, ranked

**Tier A (cheap, high value, no design risk):**

1. Add the four `set_enabled(true)` `clear_samples()` line — one-line
   correctness fix flagged at `docs/plans/perf-timing.md:1261`.
2. Add sub-spans inside `eat_and_scavenge`, `nn_forward` (sequential),
   `apply_movement_and_repulsion`, `photosynth_two_pass`,
   `run_vision_pass`. Twenty new lines total. No JSON change. Existing
   UI renders them as nested rows automatically.
3. Land the deferred `renderWorld` sub-spans (`drawSunMap`,
   `drawCarrion`, `drawCreatures`) in `web/src/render.ts`. The plan
   says they should already exist.
4. Add `tick_rate_1s()` / `tick_rate_60s()` to `WorldHandle`. Derive
   from existing profiler `tick` ring. Surface in Perf-box header.
5. Add `frame_rate_60s` + jank counter purely in `web/src/perf.ts`.

**Tier B (moderate, needs a small API addition):**

6. Add `WorldHandle::counters_json()` — births/deaths/speciations/
   extinctions per second plus carrion count plus vision-rays
   cumulative. Independent of `events_enabled` (these counters tick
   regardless; the event log just decides whether to *record* them).
7. Add `p50/p95/p99/max/last` to the profiler JSON node shape. Sort
   the (already in-memory) ring on report. Bump JSON shape version
   if anyone cares (no one downstream pins it).
8. Add `profiler_node_count` getter and surface it. Cheap insurance.
9. Span the threaded `nn_forward` post-join (per-chunk).

**Tier C (heavier; design + plan):**

10. Perf budget regression test (`tests/perf_budget.rs` +
    `tests/perf_baseline.json`). Separate plan; pin budgets first.
11. Pre-emptive event firehose hardening before re-enabling
    `events_enabled`: intern species names by id, cap `all`, add
    `recent_events_since(seq)`.
12. WASM memory + save-size readouts. Low cost but touches the
    boundary; do once with the rest.

None of A or B threatens the §16 determinism hash. All would land
behind the existing profiler-OFF default. The audit's strongest
recommendation: do all of Tier A as part of `ui-perf` or immediately
after — the Perf box is twice as useful with sub-spans and TPS, at
essentially zero cost.
