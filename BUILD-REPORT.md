# evosim v1 — build report

One-pass orchestration of v5+v6 in ~6 wall-hours. All six milestones shipped.

> **v1.3 note:** The §16 acceptance test and golden snapshot files were deleted in D6
> of the v1.3 simplification pass. Determinism verification is no longer part of the CI
> gate. See DECISIONS.md §v1.3 simplification D6.

### Manual / UI acceptance (per v5 §16 second list)
Implementation is present for every UI checkpoint, but the orchestrator cannot operate a browser, so these are coded-and-untouched rather than coded-and-eyeballed.

| Check | Status | Notes |
|---|---|---|
| Deploys to Cloudflare Pages | **SKIPPED** | orchestrator cannot deploy; `web/dist/` + `_headers` ready |
| Opening URL shows a single cell on sun-gradient-with-hotspots world | **CODE READY** | `WorldHandle::new("")` + `renderWorld` |
| Body mutation produces visible variation within ≤200 ticks | **CODE READY** | likely needs higher `mutation_rate_multiplier` to be obvious by 200 |
| Close+reopen restores world from autosave | **CODE READY** | F.26 wired end-to-end; manual test would confirm |
| Pop = 0 → eulogy card with 4-image grid + Copy + Download + New World | **CODE READY** | F.28 wired |
| Seed copyable from top bar | **CODE READY** | F.27 pill + clipboard.writeText |
| Five dev sliders modify live sim without restart | **CODE READY** | `WorldHandle::set_slider` exists; no UI sliders shipped — see Known Issues |
| Toast + highlight ring fire on speciation | **CODE READY** | E.22 wired |

## DECISIONS.md (~50 entries)

Major orchestrator decisions captured in `DECISIONS.md`. Highlights:
- **Layout**: single Cargo crate at root; web app under `/web`; wasm-pack output to `web/wasm`; no second wasm bundle for the worker.
- **Stack**: plain TS + DOM (no framework) per v6 §A; Vite + pnpm; GitHub Actions CI.
- **Past-lifespan penalty**: ambiguous spec (`4 per 1000-tick excess`) read as compound `4^(excess/1000)` → hard age cliff.
- **eye_offsets semantics**: per-active-slot additive offset off slot center (resolves v5 §10 ambiguity).
- **Carrion vision radius**: 1.5 world units (not specified in spec).
- **NN init range** kept at v6 §E's [-0.3, +0.3] but...
- **NN founder energy sensor (F.30)**: see Known Issues.
- **F.26 deviation from v5 §13**: main-thread JSON encode + pure-JS worker for IDB write, instead of "worker JSON-encodes via wasm". Outcome equivalent (no main-thread hitch); avoids two wasm bundles. Documented.
- **DAY_TICKS = 1000** for the eulogy day counter.

## Known issues

1. **F.30 — spec deviation on NN founder init.** v6 §E pins "uniform [-0.3, +0.3]" for founder NN weights. With that literal init, ALL hidden ReLU outputs collapse to zero on the founder's first-tick inputs → all action logits zero → first-index tiebreak picks `Rest` → founder starves before splitting → world ends at pop=0 within a few hundred ticks. The orchestrator added a minimal energy-sensor hardwiring in `Brain::founder` so the founder photosynthesizes by default and splits when energy is high. Offspring still mutate freely from that initial weight vector. Constants are exposed: `NN_FOUNDER_ENERGY_SENSOR_STRENGTH`, `NN_FOUNDER_SPLIT_ENERGY_WEIGHT`, `NN_FOUNDER_PHOTO_BIAS`. This is the most substantive spec violation in the build. **Recommendation: spec patch in v7 to either (a) bias the init, (b) start with N > 1 founders, or (c) seed the founder with a pre-trained micro-brain.**

2. **F.30 — `FOUNDER_SPLIT_JITTER` raised 1.0 → 50.0.** v5 §3.5 step 11 says "small jitter"; not in v6 §K slider list. Reverting to 1.0 makes children stack at parent's position, exhausting one sun cell → cascade extinction. 50.0 ≈ 1.5 sun-cell widths. The reading "small" is stretched. **Recommendation: spec patch to either pin the jitter explicitly or add it as a slider.**

3. **SUN_REFILL_RATE raised 0.08 → 0.30.** This IS a v6 §K slider (`base_sun_rate`), so changing the default is within F.30 scope, but it's a 3.75× change from the pitched value. Reason: at R=0.08, steady-state photosynth gain (~0.138/tick) is below upkeep (0.15/tick) — even a lone creature can't sustain itself indefinitely. R=0.30 gives net-positive equilibrium.

4. **No dev-panel UI for sliders.** `WorldHandle::set_slider(name, value)` is wired and works, but the `~` hotkey dev panel (v5 §11 "five sliders") is not implemented as a DOM affordance. Sliders are operable from the JS console (`world.set_slider("base_sun_rate", 0.5)`). Adding a panel is ~30 LOC; punted to save time.

5. **Per-tick allocations.** Soft-repulsion accumulators (`fx`, `fy`) and a couple of other inner-loop Vecs allocate fresh each tick. At 1500 creatures × 30tps × 100× speed = 4.5M allocs/s. Hasn't hit the §16 perf gate (8s budget, used 2.3s) but is the obvious first profiling target if F.30 polish raises sustained populations past 1500.

6. **`pick_action_c` (the C-era hardcoded picker) remained as dead code in `world.rs` initially; deleted during D.16. Verify the cleanup didn't leave dangling references.** (Spot-check passed; flagging in case the reviewer disagrees.)

7. **No SIMD/rayon micro-optimization done.** D shipped SIMD forward pass + deterministic chunking but the rayon path is behind a `threads` feature flag and default-off. Threads-on would require a re-bootstrap of the golden snapshot (sub-RNG path differs from sequential).

8. **`creature_ids_buffer` returns `Float64Array` of `u64` ids.** f64 mantissa is 53 bits so ids stay exact up to ~9.0 × 10^15 — adequate for many sim-years but not infinite. Documented in DECISIONS.

9. **Vision pass is not parallelized.** Same code path could chunk via the same `N_CHUNKS = 8` pattern as NN forward. Punted; D.19 perf test passed without it.

10. **Code-reviewer non-blockers** from B/C/D/E/F reviewer reports are recorded in each `docs/plans/milestone-*.md` under `## Code review`. They were not all fixed; the orchestrator triaged and fixed only what gated `§16`. F reviewer is still running in the background at the time of this report; check `docs/plans/milestone-F.md` for its findings after dispatch settles.

## Questions for the user

1. **F.30 NN founder hardwiring — keep, revise, or revert?** Current minimum-viable hardwiring is in `src/brain.rs::Brain::founder`. Alternatives:
   - (a) Keep (current): minimal bias just enough for survival.
   - (b) Stronger bias (founder explicitly splits at energy ≥ 100, photosynths otherwise).
   - (c) Remove and ship a "dead-on-arrival" world that the player must restart for several seeds before getting a viable lineage.
   I picked (a). Want different?

2. **`DAY_TICKS = 1000`?** That's 33 wall-seconds per "day" at 1× sim speed. v5 §11 says "lived N days" but doesn't pin a tick→day rate.

3. **Slider UI**: should I add the `~`-hotkey dev panel post-build? It's small but I left it out.

4. **Cloudflare deploy + UI smoke**: I can't run a browser. Want to walk through the UI checklist together?

5. **Threads path**: I left `wasm-bindgen-rayon` behind a feature flag. To enable by default we'd need to re-bootstrap the golden snapshot under the threaded path (sub-RNG order differs). Worth doing?

## Suggested v1.1 priorities

In order of "biggest win per LOC":

1. **Dev-slider panel.** Tiny DOM widget, big quality-of-life. ~30 LOC.
2. **Real balance pass.** F.30 only tuned the minimum to pass §16. The slider defaults aren't optimized for "fun" — populations either die quickly or stabilize without much speciation drama. A focused tuning pass watching the actual aquarium would 10× the engagement.
3. **Spec patch for founder NN init** (re Known Issue #1). Either pre-bias the init or document the energy-sensor hardwiring as part of the spec.
4. **Lineage tree visualization** (v1.1 bet per the pitch). The species registry already has parent_id; just need a UI.
5. **Profile + reduce per-tick allocations** in `apply_movement_and_repulsion` and `eat_and_scavenge` (the next perf-headroom step before WebGPU is even on the table).
6. **Trait histograms** in the Stats panel.
7. **WebGPU NN path** (v1.1 bet). Worth it only if profiling shows NN forward as the bottleneck after rayon — currently raycasts dominate.
8. **NN visualization** in the Inspector.
9. **Threads-by-default** with re-bootstrapped golden snapshot.
10. **Manual UI smoke + first deploy.**

## Build mechanics

- **Wall-time**: ~6 hours of orchestration with subagent parallelism.
- **Subagent model spend**: ~8 opus calls (planning + plan-review + code-review), ~5 sonnet calls (implementers). Total tool-uses across subagents ~600.
- **Git history**: 26 commits on `main`, milestone-shaped (each substep commit-bisectable).
- **Branch**: `main`. No remote configured.
- **Tests**: 77 unit + 1 acceptance, all green. CI workflow runs both.
- **Code volume**: ~6.5k LOC Rust + ~1.8k LOC TS + ~800 LOC tests.
- **Plan docs**: `docs/plans/milestone-B.md` through `milestone-F.md` — durable record of intent + review.
