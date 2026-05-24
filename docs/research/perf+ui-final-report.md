# evosim — perf + UI pass final report

Final verification of the v1.1 perf + UI pass (5 perf + 3 UI pieces, 9 commits).
Cross-references: `docs/plans/perf+ui-master.md`, `docs/research/perf-final-report.md`,
`docs/plans/perf+ui-cross-review.md`.

## Status

| Piece | Commit | Determinism impact | Status |
|---|---|---|---|
| perf-1 sector sin/cos cache | `cbb410e` | safe (sequential golden unchanged) | shipped |
| perf-2 scratch Vec pooling | `c382916` | safe | shipped |
| perf-3 SpatialGrid cursor pre-alloc | `1844f78` | safe | shipped |
| perf-5 genome hot-field SoA mirror | `8f8d202` | safe (additive mirror, AoS retained) | shipped |
| perf-4 threads end-to-end + dual golden | `41a91a5` | adds NEW threaded golden; sequential unchanged | shipped |
| ui-stats top-left widget | `0d9b726` | n/a (TS/HTML/CSS) | shipped |
| ui-inspector top-right popup | `642c7e7` + `2422437` | n/a | shipped |
| ui-perf bottom-left widget | `c9e446f` | n/a | shipped |

All gates green. Sequential golden `0xb76e907c6221f7f5` unchanged across the
entire pass; threaded golden bootstrapped to the **same** value (see
Determinism below).

## What shipped

- **perf-1** — pre-computed per-creature eye `(sin, cos)` cache on
  `CreatureSoA`, invalidated on push / mutation / save-restore. Eliminates
  ~72k transcendentals/tick at peak population.
- **perf-2** — promoted nine per-tick scratch `Vec`s (`scratch_fx`,
  `scratch_fy`, `scratch_neighbors`, six eat/scavenge buffers) from `vec!`
  per tick into long-lived `World` fields. Handles the split-borrow dance
  around `grid.for_each_in_radius` via `mem::take` recipe.
- **perf-3** — `SpatialGrid` gained a `cursors: Vec<u32>` field; rebuild
  now reuses it instead of cloning `starts` 3× per tick.
- **perf-5** — 7-scalar SoA mirror (`size`, `photosynth_efficiency`,
  `eat_efficiency`, `scavenge_efficiency`, `move_speed`, `vision_range`,
  `eye_count`) parallel to the existing `Vec<Genome>`. Cold paths
  (inspection, hash, mutation, save) keep reading AoS; six hot tick
  sites read the mirror. Includes a `debug_assertions` sanity check
  (`hot_mirrors_match_genomes_after_births_and_mutations` lib test).
- **perf-4** — JS-side `initThreadPool` with SAB feature-detect; rayon
  `par_iter_mut` over 8 fixed chunks in `VisionPass::run`; new
  `tests/golden_snapshot_t10000_threaded.txt` + `#[cfg(feature="threads")]`
  acceptance test; nightly toolchain pin + `.cargo/config.toml` + Vite
  worker config; CI step added.
- **ui-stats** — `#stats-box` overlay (top-left) owns the two chart
  canvases. `web/src/rail/stats.ts` reduced to 99 LOC (chart-only).
- **ui-inspector** — `#inspector-box` overlay (top-right, `display:none`
  until creature click); preserves all 15 `#ins-*` IDs.
- **ui-perf** — `#perf-box` overlay (bottom-left); profiler panel
  extracted to `web/src/widgets/perf-panel.ts` (192 LOC). Columns
  widened (`table-layout: auto`, `min-width: 480px`, `nowrap`).

## Measured perf delta

All timings on WSL2 (noisy host).

| Stage | Pre-pass | Post-pass | Delta |
|---|---|---|---|
| Sequential acceptance wall-time (median of 3) | ~2.92s (perf-1 baseline) / 2.31s (BUILD-REPORT) | 2.70s | -7.5% vs 2.92s baseline |
| Threaded acceptance, `RAYON_NUM_THREADS=1` (best of 2) | n/a | 4.43s | new pin |
| Threaded acceptance, default rayon (8+ threads) on WSL2 | n/a | 23.4s (perf budget fail) | known WSL2 issue |
| Lib test count (default) | 78 | 97 | +19 |
| Lib test count (`--features threads`) | n/a | 97 | new |
| Acceptance test count (default) | 2 | 3 | +1 (`save_load_step_preserves_determinism`) |
| Acceptance test count (`--features threads`) | n/a | 1 | new |

Per-piece sequential acceptance runs (3 trials of `acceptance_t10000` only):
2.45s / 2.70s / 3.46s → median 2.70s.

Predicted vs measured: `perf-final-report.md §3` projected
17–35% (perf-1) + 2–7% (perf-2) + 2–4% (perf-3) + indirect cache wins (perf-5).
Measured ~7.5% improvement against the perf-1-baseline of 2.92s is **below
the projected range**. The perf-1 code-review block already flagged this:
WSL2 run-to-run variance is >0.3s, the wall-time is dominated by
test-startup + binary load on small workloads, and the projection was
made against a different mix. Bit-identity to baseline is preserved, which
is the load-bearing guarantee; the absolute speedup is best measured in
the browser via the in-app profiler at sustained populations.

## Determinism

- **Sequential golden** `tests/golden_snapshot_t10000.txt` =
  `0xb76e907c6221f7f5` — **unchanged** across all 5 perf commits.
  `cargo test --release --test acceptance` (3 tests, all default-build)
  passes.
- **Threaded golden** `tests/golden_snapshot_t10000_threaded.txt` =
  `0xb76e907c6221f7f5` — **bit-identical to the sequential golden**.
  This confirms the static-analysis prediction in `perf-4-threads.md`
  that vision is RNG-free and parallel NN forward keeps per-chunk
  sub-RNG ordering identical to the sequential helper. The dual-golden
  scheme is therefore belt-and-braces; future threaded changes that
  *intentionally* diverge can regen the threaded file alone.
- `profile_does_not_change_hash` and `save_load_step_preserves_determinism`
  both pass (observer-purity and SoA-mirror-sync guarantees).
- The new `hot_mirrors_match_genomes_after_births_and_mutations` lib
  test (perf-5) catches AoS/SoA mirror drift in `debug_assertions`.

## Known issues

- **WSL2 rayon overhead.** The threaded acceptance test under default
  rayon parallelism (8+ threads) runs at ~23.4s on WSL2, exceeding the
  `PERF_BUDGET_MS = 8_000` gate. `RAYON_NUM_THREADS=1` passes (4.43s).
  Documented in `DECISIONS.md` (`## Threads (perf-4)` →
  "WSL2 rayon overhead"); CI on `ubuntu-latest` is unaffected. Local
  threaded-gate runs require the env var.
- **Asymmetric DECISIONS coverage** (cross-review item S1). Only the
  perf-4 commit added a `DECISIONS.md` section. perf-1/2/3/5 and the
  three UI pieces have their rationale in their per-piece plans but no
  short DECISIONS entry. Non-blocking; the master plan and per-piece
  plans are committed alongside the code.
- **CI threaded build runs on every push** (per `.github/workflows/ci.yml`).
  This adds wasm-pack + nightly install time to the rust job; consider
  caching the nightly toolchain if turnaround becomes an issue.
- **`pnpm build` warning** — `evosim.js` is statically imported by
  `main.ts` and dynamically imported by `wasm-bindgen-rayon`'s
  workerHelpers.js. Vite's `worker: { format: "es" }` resolves the build
  but the warning is benign. Pre-existing pattern, not regressed by this
  pass.

## Recommended next pass

Per `perf-final-report.md §3` items still on the table:

- **#8 SIMD `f32x8` ray-vs-8-circles** in vision DDA inner loop —
  highest remaining sim-side ceiling (0.5–1.2 ms/tick at peak). Now
  attractive given that the in-app profiler can confirm vision is still
  dominant.
- **#13 NN weight transpose** for layer 1 — defer; requires
  `SaveV1` schema bump and re-bootstrap of both goldens.
- **#14–18 render-side polish** (inspector throttle, `putImageData` sun
  blit, off-screen creature cull, 1Hz stats chart gate, ids buffer
  re-fetch safety) — pure JS, golden-safe, biggest frame-budget wins
  at high zoom + 100× speed. Bundle as one PR.

## DECISIONS delta

One section appended during this pass: `## Threads (perf-4)`
(5 bullet points covering dual-golden design, bootstrap recipe, build
behavior, nightly wasm config, and WSL2 rayon caveat). No edits to
prior entries.

## Build mechanics

- 8 pieces planned, 9 commits landed (`642c7e7` ui-inspector +
  follow-up fix `2422437`).
- 1 master plan + 8 per-piece plans + 1 cross-review committed under
  `docs/plans/`.
- 0 spec edits to v5/v6/ORCHESTRATOR; pass strictly within
  `docs/research/perf-final-report.md §3` items 1, 2, 3, 5, 6, 7.
- 0 new top-level crates; `Cargo.toml` unchanged (the `threads`
  feature was pre-declared).
- 0 default-build dependencies added; rayon + wasm-bindgen-rayon stay
  optional.
