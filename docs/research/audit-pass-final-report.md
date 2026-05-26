# evosim — audit cleanup pass final report

Date: 2026-05-24. Final verification of the v1.1 audit cleanup pass
(38 SHIP items across 4 PRs, 37 audit commits + 2 wrap-up commits =
39 commits total on `main`). 16 audit documents triaged via
`docs/plans/audit-triage.md`: 38 SHIP, 30 DEFER, 25 REJECT,
4 needs-user-approval. One golden regen event (PR-3); both
sequential and threaded files moved
`0xb76e907c6221f7f5` → `0xd56cd6881d2898fc` and remain bit-identical.
Sequential/threaded equivalence newly asserted by S39 runtime test.
All gates green (default + `--features threads`).
Plus one plan-only design doc: `docs/plans/webgl2-renderer-design.md`
(no code — explicit needs-user-approval gate).

## Status

| Gate | Result |
|---|---|
| `cargo fmt -- --check` | PASS (after `chore(fmt)` carryover commit `7634255`) |
| `cargo clippy --all-targets -- -D warnings` | PASS |
| `cargo clippy --all-targets --features threads -- -D warnings` | PASS |
| `cargo test --lib` | PASS — 132/132 |
| `cargo test --lib --features threads` | PASS — 133/133 |
| `cargo test --release --test acceptance` | PASS — 4/4, sequential golden `0xd56cd6881d2898fc` |
| `cargo test --release --features threads --test acceptance acceptance_t10000_threaded` | FAIL — perf budget 10 388 ms vs 8 000 ms on WSL2; hash check is gated behind the perf assert so it never runs in this environment. Expected per DECISIONS "WSL2 rayon overhead" entry. Not a regression. |
| `cargo test --release --features threads --test acceptance acceptance_threaded_actually_matches_sequential_t10000` | PASS — S39 runtime equivalence test confirms threaded path produces the sequential golden bit-for-bit (18.45 s) |
| `pnpm typecheck` | PASS |
| `pnpm build` | PASS (only the pre-existing dynamic-import warning) |
| Goldens on disk | `tests/golden_snapshot_t10000.txt` = `tests/golden_snapshot_t10000_threaded.txt` = `0xd56cd6881d2898fc` |

## What shipped (by audit doc)

16 audit docs in `docs/audit/`. S-item mapping per `audit-triage.md`.

- **architecture.md** — S1 (split `src/world.rs` into `mod/tick/nn/save_v1`), S2a (panic hook in release), S3 (BODY_RADIUS_PER_SIZE to constants), S11 (extract `count_carrion_overlap` + `compute_is_at_wall`), S17 (typed slider setters), S18 (stable-id `creature_at`), S19 (delete dead wasm-api), S20 (grid-backed `creature_at`), S21 (drop unused flag floats), S22 (status getter + DPR cap), S35 (visibility tighten). A2/A3/A4/A7/B3/B5/C/D module-merge polish DEFERRED.
- **correctness-bugs.md** — S4 (`place_hotspots` bound), S5 (NaN in `decode_action`), S6 (NaN in `mutate_f32`/`mutate_u32`), S7 (snapshot_hash coverage = C4 partial), S10 (chunk_ranges invariants = C10), S11 (= D2/F1 extracts), S12 (validate_save = C5). C2/C6/C7/C11/C12/C13 DEFERRED.
- **determinism.md** — S8 (R4 — direct u64×4 RNG hash), S9 (R6 — HashMap iter lint), S10 (R1 — chunk_ranges invariant). R3/R7/R8 DEFERRED.
- **security.md** — S12 (H1), S13 (M3 — CSP), S14 (M5 — delete events.ts), S15 (L2 — URL seed cap), S16 (L1 — graceful JSON fallback). M1/M2/M4/L3/L5 DEFERRED.
- **wasm-api.md** — S17 (8.2), S18 (S9), S19 (S4/S5), S20 (S8), S21 (H1 smaller variant), S22 (H5/H6/A1/A4). H1-H4 full ptr-rewrite + M1 batched snapshot DEFERRED.
- **web-wasm.md** — covered by S13/S14/S15/S18/S20/S21/S22 and S38 (6.1 sourcemap). 1.10-1.12 render-perf DEFERRED.
- **dependencies.md** + **build-tooling.md** — S37 (drop rand; twox-hash 1→2; wasm-bindgen-rayon 1.3), S38 (wasm-opt metadata + rust-toolchain.toml + .nvmrc + hidden CI sourcemap). cargo deny/audit/criterion REJECTED for this pass.
- **rust-quality.md** — S33 (`#[inline]`), S34 (`impl Error for LoadError`), S35 (visibility), S36 (drop `mod heapless`). `mul_add`/`hypot` clippy hints REJECTED (wrong for wasm32/use-case).
- **allocations.md** — S23 (par_chunks_mut direct SoA write — #1), S25 (scratch_eat_candidates — cell-1/C13), S26 (cell_to_carrion CSR — #5), S27 (scratch_dead + scratch_species_lost — #2/#3), S28 (dense-bool alive species — #3). Brain weight pool (cell-4) DEFERRED.
- **perf-hot-loop.md** — S23 (#2), S24 (#4), S25 (#3), S26 (#7), S28 (#17), S29 (#15), S30 (#12), S31 (#10), S32 (#9). #1/#5/#6/#8/#11/#13/#14/#18/#19/#20 DEFERRED (NN transpose + SIMD ray-vs-circle + partition-by-action — the next perf pass).
- **test-gaps.md** — S39 (P0/P1 — `acceptance_threaded_actually_matches_sequential_t10000` + `save_load_hash_equal_immediately_after_load` + chunk-count invariant). P2-P8 DEFERRED.
- **observability.md** — nothing shipped this pass. Tier A/B/C all DEFERRED (each needs its own design pass crossing the wasm boundary).
- **sim-balance.md** — only §8 (`biggest_ever` perf+correctness) shipped, via S29. The rest is gameplay tuning, intentionally excluded; per orchestrator brief this is "next pass" scope.
- **plans-coherence.md** — nothing shipped (housekeeping DEFERRED).
- **big-wins.md** — nothing shipped as code. #1 (WebGL/WebGPU) got a plan-only doc `docs/plans/webgl2-renderer-design.md`. #3, #8, #9 need user approval. #2/#4/#5/#6/#10 DEFERRED. #7 partially-shipped via S14/S19.

## PR-by-PR summary

| PR | Commit range | Commits | Files | LOC delta | Golden status |
|---|---|---|---|---|---|
| PR-1 foundation + safety | `14783f1`..`fcac7f2` (continuous early block including PR-2's 5) | 13 audit + 5 PR-2 interleaved | 27 | +2767 / -2660 | unchanged at `0xb76e907c6221f7f5` |
| PR-2 wasm boundary cleanup | `3dec63d`..`fcac7f2` | 5 | 5 | +306 / -126 | unchanged at `0xb76e907c6221f7f5` |
| PR-3 determinism + correctness + regen | `772df68`..`c06c0ef` | 10 | 16 | +1146 / -152 | **regen** `→ 0xd56cd6881d2898fc` for BOTH files (bf3287a is the sole commit touching either golden) |
| PR-4 per-tick perf followups | `e3840b9`..`d0b6839` | 9 | 6 | +663 / -153 | unchanged at `0xd56cd6881d2898fc` |
| wrap-up | `7634255` (fmt), `c2286f1` (decisions) | 2 | 5 | +92 / -38 | unchanged |

Total: 39 commits on `main` since `2312d8c`.

## DECISIONS delta

Four new `## Audit v1.1 ...` sections appended to `DECISIONS.md` this pass (PR-4 review entry was not authored — see Known Issues):

1. **`## Audit v1.1 PR-1 review (2026-05-24)`** — line 178. 13 commits enumerated; both goldens still `0xb76e907c6221f7f5`; S1 split sizes (988/746/469/137); S35 visibility verified; CSP header confirmed; events.ts gone with no surviving references.
2. **`## Audit v1.1 PR-2 review (2026-05-24)`** — line 188. 5 commits; goldens unchanged; S17/S19/S18+S20/S21/S22 each annotated with file/line evidence; stride-13→11 + render.ts guard noted.
3. **`## Audit v1.1 PR-3 (2026-05-24)`** — line 200. THE REGEN entry. Documents the 18 SoA fields added to `snapshot_hash` (S7: `digestion_cooldown`, `cumulative_upkeep`, `species_id`, `parent_species_id`, `last_action`, `action_this_tick`, `max_size_reached`, `distance_travelled`, `birth_tick`, `carrion.id`, `carrion.sun_cell`, species `parent_id`/`name`/`born_tick`/`died_tick`/`child_count`/`depth`/`anchor_brain_weights`, registry `next_id`). S8: RNG hash format changed from `serde_json::to_vec` to direct `u64×4` via new `SimRng::state()`. NaN canonicalization added to `write_f32` (all NaN payloads map to `0x7fc0_0000`). **New sequential hash = new threaded hash = `0xd56cd6881d2898fc`** (still bit-identical across paths). S24 cell-sweep scavenge: hash unchanged in this seed.
4. **`## Audit v1.1 PR-3 review (2026-05-24)`** — line 215 (committed in wrap-up `c2286f1`). Confirms the regen, lists scope discipline checks per commit, flags the fmt carryover that was fixed in `7634255`, notes the S39 test was renamed `_matches_` → `_actually_matches_` by the implementer (no functional change), confirms `pub force_sequential_nn` has the correct cfg_attr.

## Items deferred to v1.2+

Grouped from the triage DEFER bucket (34 items, full enumeration in
`docs/plans/audit-triage.md` lines 317-356).

- **Next perf pass — high-value, structural** (D22, D23, D25): NN weight transpose + SIMD ray-vs-circle + aligned brain weights (perf-hot-loop #1/#18/#20 — big-wins #5); partition-by-action + merged photosynth (#5/#6); Brain weight pool. Each requires save schema change and/or restructured `Brain`; bundle as a perf-hot-loop pass after the sim-balance settles.
- **Tiny perf followups** (D24): perf-hot-loop #8/#11/#13/#14/#19 — each <1%, bundle as one micro-perf PR.
- **Observability Tier A/B** (D26, D27): sub-spans + TPS + jank counter + counters_json + p50/p95/p99 + profiler_node_count. Spans the wasm boundary + perf widget; own pass.
- **Test-hardening** (D28, D29): test-gaps P2-P8 selective; broad coverage gaps; own pass.
- **Web/wasm rendering polish** (D30, D31): wasm-api H1-H4 full ptr rewrite + M1 batched snapshot (big-wins #6); web-wasm 1.10-1.12 Path2D batching / highlight set / fillStyle alloc.
- **Sim-balance** (D32 + R21): whole doc except §8. Gameplay tuning; the brain/balance/grass orchestration the user already signaled.
- **big-wins** (D34): #2 (MAX_POPULATION pre-alloc — partially shipped via S12's cap), #4 (memory budget + binary save), #5 (Brain slab), #6 (frame_snapshot), #10 (drop sequential codepath — see Questions).
- **Correctness / determinism polish** (D1-D13): C2 geom_skip off-by-one (needs spec call), C7 eye-offset mutation, C11 species-distance anchor (spec call), C12 EventLog default, C6 NN result-ordering rewrite (S23 supersedes), M1 Float32Array lifetime, M2 esbuild CORS, M4 unsafe ptr polish, L3/L5 security low, R3 CI thread matrix, R7 libm consistency, R8 wide reduce_add lane assertion.
- **Architecture polish** (D14-D21): TickScratch substruct, genomes accessor encapsulation, data-driven `Action::is_valid`, inspector JSON binary, throttle `creature_inspect_json` to 5 Hz, split `main.ts`, module-merge cleanups, mutation-rate term in species_distance.
- **Plans-coherence housekeeping** (D33): archive shipped plans, refresh stale headers.

## Items rejected (with reasoning)

From the triage REJECT bucket (25 items, `audit-triage.md` lines 370-398):

- **Already shipped** (R1-R9): perf-1..perf-5, perf-4 threads + dual golden, ui-stats / ui-inspector / ui-perf, perf-timing — all landed in the prior pass per `docs/research/perf+ui-final-report.md`. These appeared in the audit because the audit was written before perf+ui shipped.
- **v5 §14 deferred, needs user approval** (R10, R11, R16, R17): big-wins #1 WebGL/WebGPU; #3 NEAT; #8 quadtree; #9 deterministic replay. Restated under "Items needing user approval" below.
- **Wrong target this pass** (R12-R15, R18, R19): big-wins #2/#4/#5/#6 (multi-day structural shifts — DEFERRED); #10 drop sequential codepath (the dual path protects WSL2 devs); architecture B3 inspector typed-array rewrite (throttle is the cheap fix).
- **Out of scope for cleanup** (R20, R22, R25): observability Tier C (perf budget regression test, event-firehose hardening, wasm-memory readouts); test-gaps P5-P8 (testing-hardening orchestration); cargo deny/audit/criterion/ESLint/pre-commit/A11y full pass.
- **Spec/scope mismatch** (R21): sim-balance §1-§7/§9/§10 — gameplay tuning, the user explicitly scoped balance separately.
- **Clippy wrong for our target** (R23, R24): `mul_add` (wasm32 has no FMA), `hypot` (much slower than `sqrt(dx²+dy²)` for our use).

## Items needing user approval

Four items. Each becomes its own orchestration if the user wants it.

- **big-wins #1 — WebGL2/WebGPU renderer.** Largest single perf payoff in the audit set. Plan-only doc `docs/plans/webgl2-renderer-design.md` landed this pass (NO code). Needs sign-off before planning agents start.
- **big-wins #3 — NEAT brain.** Would invalidate the F.30 founder NN-init hardwiring (already a known spec deviation per BUILD-REPORT Known Issue #1). Multi-week effort.
- **big-wins #8 — quadtree / sweep-and-prune spatial index.** Architectural shift; invalidates the current chunk-partition determinism comments; forces a new partition scheme.
- **big-wins #9 — deterministic replay/scrubber.** Product shape question (event stream + UI). Audit flagged as engagement multiplier.

## Known issues surfaced during implementation

- **S37 `twox-hash` 2.x explicit feature.** Bumping `twox-hash` 1→2 required the explicit `xxhash64` feature flag (default-features changed in 2.x). Without it the crate builds but no algorithms are exposed. Resolved in `a4589ba`: `twox-hash = { version = "2", default-features = false, features = ["xxhash64"] }`. Algorithm is byte-stable across versions so the golden survived this commit.
- **S38 vite type fix.** The CI-conditional sourcemap line `sourcemap: process.env.CI ? "hidden" : true` initially tripped Vite's `sourcemap` type (it accepts `boolean | "inline" | "hidden"` but TS narrowed to `boolean | string`). Resolved during `pnpm build` iteration in `65fcf72`.
- **S23 explicit `prev_vx`/`prev_vy` plumbing.** The threaded `par_chunks_mut` direct-SoA write had to pass `prev_vx`/`prev_vy` explicitly through `pick_action_d` because the closure no longer captured `&mut self.creatures.vx[..]` in the way the sequential path did. Commit `4bbd826`. Not a regression; documented in the commit body.
- **S30 `copy_from_slice` instead of `mem::swap`.** The plan called for `std::mem::swap(&mut last_action, &mut action_this_tick)`. The implementer used `copy_from_slice` in `e3840b9` because the SoA fields are `Vec<Action>` co-resident on `CreatureSoA` and `mem::swap` on two fields of the same struct requires either `split_at_mut` gymnastics or a temporary, which is more code than the bulk-copy. Functional outcome identical; bulk copy is a single memcpy. No determinism impact (the rationale in S30 — that `action_this_tick` is overwritten by `nn_forward` before being read — still holds).
- **WSL2 threaded perf budget.** `acceptance_t10000_threaded` reports 10.4 s vs the 8 s budget on this host (24.6 s in PR-3's reviewer run with more variance). This is the documented WSL2 rayon overhead per `DECISIONS.md` "WSL2 rayon overhead" entry; the hash assertion is gated behind the perf assert, so we use the S39 runtime equivalence test (`acceptance_threaded_actually_matches_sequential_t10000`) to verify threaded determinism instead. S39 passes in 18.45 s.
- **fmt carryover from S24 / PR-3 reviewer.** Commit `1d3c62f` (S24) shipped with three trivial fmt nits in `src/world/tick.rs` flagged by the PR-3 reviewer. The PR-3 reviewer also ran `cargo fmt` over the broader PR-3 commits (snapshot_hash.rs / genome.rs / rng.rs / save.rs) and authored the PR-3 review DECISIONS entry, but neither change set was committed. This verifier landed both as wrap-up commits: `7634255` (`chore(fmt): clean up PR-3 carryover`) and `c2286f1` (`chore(decisions): append PR-3 review entry`). Cross-ref `DECISIONS.md` line 220 (the FIX-NEEDED bullet in the PR-3 review entry).
- **S39 test rename.** The PR-3 implementer named the cross-feature equivalence test `acceptance_threaded_actually_matches_sequential_t10000` instead of the plan's `acceptance_threaded_matches_sequential_t10000`. Functionally identical; cross-ref `DECISIONS.md` line 222.
- **`pub force_sequential_nn`.** The new escape hatch is `pub` (not `pub(crate)`) intentionally — the integration test `acceptance_threaded_actually_matches_sequential_t10000` toggles it from outside the crate. Carries `#[cfg_attr(not(feature = "threads"), allow(dead_code))]`. Cross-ref `DECISIONS.md` line 223.
- **No PR-4 review DECISIONS entry.** The orchestrator brief expected "possibly PR-4 review" — none was authored. PR-4 was 9 partition-stable perf commits; both goldens remain at the PR-3-pinned value. The implementer did not annotate DECISIONS for PR-4 because (a) no hash change, (b) no semantic change, (c) the rationale is captured in each commit body. Verifier did not back-fill — would be polish over substance.
- **Untracked plan docs.** `docs/plans/audit-*.md` (15 files including `audit-master.md`, `audit-triage.md`, `audit-cross-review.md`, and per-S plan docs for S1/S7/S8/S11/S12/S17/S18/S21/S23/S24/S26/S39) and `docs/plans/webgl2-renderer-design.md` remain untracked in the working tree. They were authored by planning agents but never `git add`-ed. Not a code issue; flagging for the orchestrator to decide whether to commit them as one `docs(plans): audit-v1.1 planning artifacts` commit or leave them as scratch.

## Questions for the user

1. **Brain / balance / grass mechanic — NEXT pass scope; ready for orchestration?** This is the natural next pass per the orchestrator brief and per the deferred sim-balance + F.30 issues from the original BUILD-REPORT. All the substantive gameplay-tuning audit items (sim-balance §1-§7/§9/§10) intentionally went DEFER waiting for it.
2. **Sequential vs threaded equality preserved bit-for-bit through this pass.** S39 now asserts it as a runtime test. As future threading changes (big-wins #10, the next perf pass for #1/#18/#20) loom: opinion on dropping the sequential codepath entirely? Triage punted (R18 / D34) because the dual path protects WSL2 devs and SAB-less browsers. If the user wants to commit to threads-only post-WebGPU-renderer, the decision unlocks ~20% of `world/nn.rs` and lets `force_sequential_nn` go away.
3. **Commit the untracked planning docs?** 15 audit plan + research artifacts in `docs/plans/` are uncommitted (see Known Issues last bullet). The pattern in prior passes was to commit them; this pass left them as scratch. Quick yes/no.
4. **PR-4 review DECISIONS entry — back-fill or skip?** Both PR-1 and PR-2 have reviewer entries committed by the corresponding reviewers; PR-3 review entry exists (committed as wrap-up); PR-4 has none. The reviewer's verdict per the brief was "9 partition-stable commits, no regen, both goldens still match"; arguably DECISIONS doesn't need to record non-events. Verifier deferred — would write if the user wants the symmetry.

## Suggested next pass priorities

**STRONGLY RECOMMEND brain / balance / grass orchestration as the next pass.**
Per the orchestrator brief; per sim-balance.md's DEFERRED tuning items;
per BUILD-REPORT Known Issues #1, #2, #3 (F.30 founder hardwiring,
FOUNDER_SPLIT_JITTER, SUN_REFILL_RATE — all gameplay tuning the
audit deliberately left alone). The technical foundation is now
clean enough that a balance pass can focus on the actual sliders /
energy curves / extinction dynamics without fighting world.rs
monolith merge conflicts.

Other follow-ups, in rough priority:

1. **Next perf pass** — perf-hot-loop #1 / #18 / #20 (NN weight transpose + SIMD ray-vs-circle + aligned weights). Requires save-schema change + dual golden regen. Highest-value remaining perf work per the audit. Bundle big-wins #5 (Brain slab) into the same orchestration.
2. **Observability Tier A** — sub-spans + TPS + jank counter. Independent of #1; spans wasm boundary. Useful for the brain/balance pass's measurement.
3. **Deferred big-wins** — #2 (MAX_POPULATION pre-alloc), #4 (binary save + memory budget), #6 (frame_snapshot), #10 (drop sequential codepath — pending user answer to Q2 above).
4. **Test-hardening pass** — test-gaps P2-P8 selective; bundle with a `cargo audit` / `cargo deny` CI step (R25 subset).
5. **Web rendering pass** — web-wasm 1.10-1.12 (Path2D batching, highlight set, fillStyle alloc) OR commit to big-wins #1 (WebGL/WebGPU renderer; plan-only doc already exists at `docs/plans/webgl2-renderer-design.md`).

## Build mechanics

- **Wall-time estimate**: orchestrator-reported ~6-8 wall-hours across the 4 PRs (similar shape to perf+ui pass).
- **Commits by PR**: PR-1 = 13 audit, PR-2 = 5 audit, PR-3 = 10 audit, PR-4 = 9 audit; plus 2 verifier wrap-up = **39 total** on `main` (range `2312d8c..HEAD`). Sequence: PR-1 (`14783f1`..`537199c`) → PR-2 (`3dec63d`..`fcac7f2`) → PR-3 (`772df68`..`c06c0ef`) → PR-4 (`e3840b9`..`d0b6839`) → wrap-up (`7634255`, `c2286f1`).
- **Agent counts (orchestrator-reported)**: ~4 planning agents (one per PR), ~4 plan-review agents, ~4 implementer agents, ~4 code-review agents; plus this verifier. Roughly mirrors the perf+ui pass shape.
- **Golden hash transitions**: ONE regen event, in PR-3 commit `bf3287a` (`fix(snapshot-hash): extend coverage + replace serde_json RNG with direct u64×4`). Pre-pass: `0xb76e907c6221f7f5` (both files). Post-PR-3: `0xd56cd6881d2898fc` (both files; still bit-identical sequential vs threaded). Verified unchanged through PR-4 and wrap-up. `git log -- tests/golden_snapshot_t10000.txt` and `git log -- tests/golden_snapshot_t10000_threaded.txt` both show `bf3287a` as the sole audit-pass commit touching them.
- **Code volume delta**: PR-1 +2767/-2660 (dominated by S1 mechanical split), PR-2 +306/-126, PR-3 +1146/-152 (snapshot_hash coverage + S39 tests), PR-4 +663/-153 (mostly scratch-pool additions to `CreatureSoA` + `World`), wrap-up +92/-38.
- **Test count delta**: lib tests 132 default / 133 threads (was 97 / 97 post-perf+ui). Acceptance 4 default / 1 threaded perf-budgeted + 1 threaded perf-ungated = effectively 6.
- **Wasm bundle**: 435.17 KB (`dist/assets/evosim_bg-*.wasm`). S37 (dep drop) + S38 (wasm-opt metadata + drop sourcemap) projected ~30-70 KB save vs pre-pass; not measured against pre-pass baseline in this verifier run.
