# Plans coherence audit

Date: 2026-05-24. Scope: `docs/plans/*.md`, `README.md`, `DECISIONS.md`,
`BUILD-REPORT.md`. Reconciles the perf+UI master + cross-review with what
recent commits actually shipped.

## Shipped vs planned (perf track)

All five perf pieces have landed on `main`:

| piece | commit | plan state | reality |
|---|---|---|---|
| perf-1 sector trig | `cbb410e` | plan + plan-review + code-review (approved) | shipped, `CreatureSoA.eye_trig` present |
| perf-2 scratch pool | `c382916` | plan + code-review (approved) | shipped, 9 `scratch_*` fields on `World` |
| perf-3 grid cursor | `1844f78` | plan + plan-review + code-review (approved) | shipped, `SpatialGrid.cursors` present |
| perf-5 genome SoA | `8f8d202` | plan exists | shipped, 7 `g_*` mirror fields present |
| perf-4 threads | `41a91a5` | plan exists | shipped, `init_thread_pool` + parallel vision + dual golden |

Master plan §4 commit ordering (1→2→3→5→4) was honored exactly. Dual
golden file `tests/golden_snapshot_t10000_threaded.txt` exists.
`DECISIONS.md` has the `## Threads (perf-4)` section (with a useful
additional WSL2 perf note absent from the plan).

## Shipped vs planned (UI track)

**None of the three UI widget plans (ui-stats, ui-inspector, ui-perf)
have shipped.** Verified:

- `web/index.html` still has `<aside id="right-rail">` visible, with
  `#chart-pop` / `#chart-species` inside `#rail-stats` and
  `#profiler-panel` inside `#rail-stats`.
- No `#stats-box`, `#inspector-box`, or `#perf-box` exist.
- `web/src/widgets/` directory does not exist.
- `web/src/main.ts:5` still imports `installProfilerPanel from
  "./rail/stats"` (old path).
- No `.overlay-widget` CSS class.

This means the perf+UI master plan is half-executed — the perf half is
done, the UI half is untouched.

## Contradictions

1. **`docs/plans/perf-timing.md` "Code review — FIXES REQUIRED" block
   (lines ~1240–1263) is stale.** That review was written against
   commit `e334039` and listed three blocking issues (missing UI,
   wrong clock source). Both were fixed in follow-up commits `b95c459`
   (Performance.now clock) and `08ad68d` (Stats-panel UI). The doc
   still reads as if the fixes are pending. Recommend appending a
   "resolution" note or moving the block to an archive section so
   future readers do not chase already-fixed bugs.

2. **`README.md:29` says `wasm-pack build … --features threads`** —
   correct per perf-4 §2g. But `README.md:58` lists tests as
   `cargo test --release --test acceptance` only, with no mention of
   the threaded acceptance command (`--features threads`). The dual-
   golden contract pinned in `DECISIONS.md ## Threads` implicitly
   requires running both. Minor — README undersells the test surface.

3. **`BUILD-REPORT.md` "Known issues" #5 (per-tick allocations), #7
   (no SIMD/rayon), #9 (vision pass not parallelized)** are all now
   fixed (perf-2 and perf-4). The report is a v1 snapshot and was
   never intended to be live; treat as historical. Recommend adding
   a one-line "superseded by v1.1 perf pass — see
   `docs/plans/perf+ui-master.md`" pointer at the top of the Known
   Issues section.

4. **Master plan §"Status at handoff" and per-piece "Status: plan
   only"** — every perf plan still self-labels "plan only. No code
   lands until this is signed off." even though all five have shipped
   and have appended code-review blocks confirming approval. The
   header status line in each plan should be updated to "shipped"
   (or the plan files moved to an archive directory) to avoid future
   confusion. UI plans correctly remain "plan only".

5. **`perf-timing.md` itself is mis-categorized.** The doc-comment
   says `Status: plan only. No code lands until this is signed off.`
   but the entire profiler ships (and its code review is appended).
   Same status-staleness as #4.

## Overlaps / duplicate work

1. **Master plan §9 "Cross-review findings" duplicates
   `perf+ui-cross-review.md` entirely.** §9 is a 5-paragraph executive
   summary; the sibling file is the full review. Either is fine alone;
   keeping both means future edits must update two places. Recommend
   converting §9 into a 2-line pointer ("see `perf+ui-cross-review.md`
   for must-fix M1–M5 and verdict tables") and letting the sibling
   own the content.

2. **`perf-1` and `perf-5` both edit `CreatureSoA::push` /
   `with_capacity` / `remove_indices`.** Properly documented as merge
   notes in both plans, and both shipped clean. Not a real overlap
   anymore — file as resolved.

3. **`ui-stats` and `ui-perf` both share `web/src/rail/stats.ts`.**
   Cross-review §F confirms the planned split is clean (lines 1–99 to
   ui-stats, 101–270 to ui-perf). No action needed; just a flag for
   the implementer.

## Gaps (missing plans for known follow-up work)

1. **No "perf-4 was perf-item #5 + #6"; perf-item #8 (SIMD
   ray-vs-circle batch) and #13 (NN weight transpose) are explicitly
   deferred** in `perf-final-report.md` and reiterated as out-of-scope
   in perf-4 §8. If those are intended for v1.2, no plan exists yet.
   Lowest-effort fix: add a `docs/plans/_backlog.md` listing deferred
   perf items #8, #9 (pre-alloc `MAX_POPULATION`), #13 with one-line
   rationale each.

2. **No "perf-4" numbering gap** — user prompt suspected perf-4 was
   missing, but `docs/plans/perf-4-threads.md` exists and shipped.
   The numbering is dense (1, 2, 3, 4, 5). False alarm.

3. **Per-piece `MAX_POPULATION` pre-allocation** is referenced as
   `perf-final-report §6 commit 4` in perf-2 §1.3 (item #9) and
   perf-2 §10 as a "compatible follow-up" but has no plan file. If
   it's intended to land it needs its own plan (or an explicit
   "wontfix").

4. **Eat-path `candidates: Vec<usize>` pooling** — perf-2 §10
   explicitly defers it. Same gap as above.

5. **`collect_deaths` Vecs (`dead`, `species_lost`)** — perf-2 §2
   explicitly defers. Same gap.

6. **No plan for the dev-slider panel** (BUILD-REPORT Known Issue #4,
   "Suggested v1.1 priorities" #1). It's been "punted to v1.1" since
   the initial build and remains unplanned despite being a top
   priority.

7. **Profiler R8 (atomic-load overhead docstring), R11 (drop "roll
   /call" column), R12 (parallel-chunk profiler invariant)** are
   "should-fix" items in `perf-timing.md` that may have been
   addressed in `08ad68d`'s UI commit but are not separately tracked.
   Worth a 1-line confirmation in `perf-timing.md`.

## Unclear priorities

1. **UI track order.** Master §4 recommends ui-stats first
   (it owns the canonical `.overlay-widget` and the
   `#right-rail { display: none }` rule), then ui-inspector and
   ui-perf interchangeably. This is in the plan but not in any
   roadmap doc — a casual reader of `BUILD-REPORT.md` won't know
   the UI pieces have a sequencing constraint.

2. **Perf vs UI PR cadence.** Master §4 suggests "commits 1–4 form a
   coherent perf PR; commit 5 is its own PR; commits 6–8 are a UI
   PR." All five perf commits landed directly on `main` without
   intermediate PRs. The UI track has no clear go/no-go gate. Is the
   UI pass blocked on user sign-off, or just unscheduled?

## Plans without clear acceptance criteria

Every perf plan has an explicit "tests must stay green" gate (golden
hash + clippy + pnpm build). The UI plans (`ui-stats`, `ui-inspector`,
`ui-perf`) have softer gates:

- ui-stats §1.6: "No new chart library, no framework, no styling lib."
- ui-inspector §"Tests": "No Rust tests touched. `pnpm build` must
  stay clean." — but no visual/behavior acceptance.
- ui-perf §"Tests": same shape as ui-inspector.

There is no enumerated "the user can do X" test plan for the UI track.
Recommend appending a `## Acceptance` section to each UI plan listing
3–5 manual checks (e.g. "creature click pops top-right inspector;
empty-canvas click dismisses; checkbox toggle still drives polling").
Not blocking, but the master plan's UI verdict will otherwise be
subjective.

## Suggested merge / split actions

1. **Archive shipped plans.** Move
   `perf-1-sector-trig.md`, `perf-2-scratch-pool.md`,
   `perf-3-grid-cursor.md`, `perf-5-genome-soa.md`,
   `perf-4-threads.md`, and `perf-timing.md` to
   `docs/plans/shipped/` (or similar). They are no longer
   actionable; their value is historical. Keep `perf+ui-master.md`
   and `perf+ui-cross-review.md` in place since the UI half is still
   pending.

2. **Update master `perf+ui-master.md` §"Status"** at the top: the
   perf half is done; only ui-stats / ui-inspector / ui-perf are
   active. Strike (or annotate "DONE") the perf rows in the §2
   inventory table. Strike or fold §9 into the sibling cross-review
   file.

3. **Split or unify `BUILD-REPORT.md`.** It's a snapshot of v1
   handoff; v1.1 perf has invalidated multiple Known Issues. Either
   freeze it (add a header "as of v1.0; superseded sections marked")
   or extract the still-valid recommendations into a live
   `docs/v1.1-priorities.md`.

4. **Create `docs/plans/_backlog.md`** for the deferred follow-ups
   (perf items #8, #9, #13; eat candidates pool; collect_deaths
   pool; dev-slider panel; trait histograms; lineage tree;
   NN visualization). One-line entries with status (`deferred`,
   `wontfix`, `v1.2`).

5. **Refresh `perf-timing.md`'s "Code review — FIXES REQUIRED"
   block** to reflect that both blocking issues are fixed in `b95c459`
   and `08ad68d`. Or delete that block and keep only the approval
   summary.

6. **Add an `## Acceptance` section to each UI plan** with 3–5
   manual smoke checks. Same shape across all three for review
   consistency.

## TL;DR

- Perf track: complete and clean. Only stale `Status: plan only`
  headers and the perf-timing "FIXES REQUIRED" block need a refresh.
- UI track: untouched. Master plan correctly identifies the
  ui-stats → ui-inspector / ui-perf ordering; no code has run.
- Cross-review M1, M2, M3 (perf merge notes) all came true and
  landed clean.
- Cross-review M4, M5 (`.overlay-widget` `pointer-events` policy) are
  still pending — they fire only when UI work begins.
- No real contradiction between master and cross-review docs.
  Main risk going forward is the half-executed `perf+ui-master.md`
  reading as if all eight pieces are open when only three are.
- "perf-4 missing" was a false alarm; `docs/plans/perf-4-threads.md`
  exists and shipped as `41a91a5`.
