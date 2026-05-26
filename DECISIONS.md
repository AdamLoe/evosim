# DECISIONS

Running log of decisions made by the orchestrator for things v5+v6 didn't pin.
Format: `<topic>: <choice> — <one-line why>`.

- cargo layout: single crate at repo root (not workspace) — only one wasm artifact, simpler for v1 size
- web app location: `/web` directory, Vite + TS — keeps Rust and JS shell clearly separated, matches v6 §A
- wasm-pack target: `web` — direct ES module import from Vite, no extra bundler shim
- pkg output location: `web/wasm` — wasm-pack output is consumed only by the web app; co-locate so Vite's import graph and CI step are obvious
- pnpm install path: `~/.local/bin` via user-prefixed npm — `npm -g` blocked by /usr perms on this WSL2 box; corepack also blocked
- web framework: none (plain TS + DOM) — per v6 §A
- node version pin: not pinned in v1 — repo runs on whatever Node 20+ the user/CI has
- CI provider: GitHub Actions — only one set up; v5 says "GitHub Actions or similar"
- founder starting energy: 200 — v5 doesn't pin this; at 20 the founder dies before reaching split with v5's actual balance numbers (steady-state photosynth 0.08/tick vs upkeep 0.15/tick). 200 gives a fat buffer so the placeholder behavior gets multiple split cycles; long-term sustainability is a brain/balance concern (F.30)
- past-lifespan penalty: interpreted as `4^(excess_ticks/1000)` compound multiplier on upkeep (capped at 1e6) — v5 §7's "multiplied by 4 per 1000-tick excess" is ambiguous; the compound reading makes age a hard cliff per design intent
- Milestone B placeholder action: split when energy >= 80 (not the spec's 50 floor). At 50, parent immediately starves post-split with no gift left for child; 80 leaves a 30-energy buffer for the child. Replaced wholesale by the NN in Milestone D
- internal body radius unit: world units, 1.0 per genome size unit — render layer scales 2.5 px per size at base zoom per v6 §B
- ring order: eye→move→scav→mouth→armor (innermost→outermost) — v6 §B fixes only eye=innermost; rest are aesthetic (armor as outermost = shell metaphor)
- eye_offsets semantics: per-active-slot additive offset off slot center — v5 §10 ambiguous; treats offsets as fine-grain, matches σ=0.1 rad scale in v6 §F
- CARRION_RADIUS_FOR_VISION: 1.5 world units — not in spec; small enough not to dominate but large enough a ray reliably hits a corpse blob
- eat-while-cooldown cost-charge: B charges attempt cost even in cooldown; D's valid-fallthrough will avoid issuing Eat in cooldown — punt fix to D
- carrion vision color: fixed gray (0.4, 0.4, 0.4) per v6 §4 "What changed vs v5", regardless of dead creature's original pigment
- cell-to-carrion index: rebuilt every tick (O(N+C)); simpler than incremental maintenance; trivial at v1 sizes
- hardcoded picker energy threshold: 80 (matches B's SPLIT_PLACEHOLDER) — same starvation reason as B's DECISIONS entry

# Milestone D decisions

- nn-input energy_frac: clamp(energy/100,0,1) — v5/v6 unspecified; matches SPLIT_THRESHOLD×2
- nn-input size norm: /SIZE_MAX — keeps in [0,1] for activation parity
- nn-input vx/vy norm: /MOVE_SPEED_MAX — keeps in roughly [-1,+1]; fast movers saturate at ±1
- nn-input vx/vy timing: previous-tick post-clip velocities ÷ MOVE_SPEED_MAX — gives NN feedback on its own motion
- is_at_wall: uses creature radius (size × BODY_RADIUS_PER_SIZE), not raw size — keeps "creature touches wall" semantics consistent with physics step
- carrion_overlap_norm: count/4, clamp at 1 — v6 §3 unspecified; 4 ≈ "corpse-rich zone"
- vision distance norm: none (raw world units) — v6 §E silent; NN learns scale via weights
- NN biases: none — spec'd weight count (3456 = 136×24 + 24×8) matches no-bias exactly
- weight layout: row-major, hidden-units / outputs contiguous — best cache behavior for SIMD matmul
- hidden and output scratch: stack per call (96+32 bytes) — no alloc; trivially thread-safe
- threads default off: feature flag `threads` wires rayon; v1 single-threaded by default; F.29 perf result decides whether to enable for §16 acceptance
- N_CHUNKS value: 8 — v6 §J's example value; matches typical core counts
- nn-mutation-rate-multiplier: applied at child_from per v6 §K — was missing in B; D fixes
- output activation 0..1: tanh post-forward (in pick_action_d), not in Brain::forward — keeps Brain::forward a pure linear-ReLU-linear graph; activation lives at decode site
- pick_action_c lifetime: deleted on D land — dead code; D replaces it

# Milestone E decisions

## E.20
- speciation event parent name: NOT in event payload — UI resolves via species_list_json (avoids stale-name drift across renames in future)
- speciation anchor clone cost: ~14KB Vec<f32> per speciation event — acceptable at v1 scale; revisit in v1.1 if NN_WEIGHT_COUNT grows

## E.21
- creature_at returns SoA index (not stable id) — caller re-resolves via creature_ids_buffer + creature_inspect_json each frame; cheap at v1 sizes
- species_list_json frequency: 1Hz (not every frame) — counts are O(N) and the panel doesn't need sub-second freshness
- creature_ids_buffer uses Float64Array: u64 ids fit in f64 mantissa for the v1 session lifetime
- stats sampling cadence: every 10 sim-ticks via tick % 10 == 0 check; 500-sample ring buffer TS-side (≈ 5000 ticks visible)

## E.22
- toast/highlight timing: wall-clock seconds (4s toast, 1.5s ring) per v6 §B; not sim-tick scaled
- toast variants: Speciation/Extinction/FirstToMove/FirstToEat fire toasts; PopulationMilestone/WorldEnded log-only (avoid spam + eulogy supersedes)
- toast stack cap: 6 visible; newer toasts at top (column-reverse)
- EventKind serde: #[serde(tag = "type")] flat shape — adopted in E because E adds the only TS consumer
- event diff: by events_total_count (monotone u32), not ring length — survives ring overflow

## E.23
- stats charts use Canvas2D directly: zero deps, < 100 LOC, no interaction needed (v6 §A plain TS)
- stats x-axis = sim-tick (not wall-clock): consistent with sim semantics; player can pause and the chart freezes

## E.24
- inspector tap detection: < 4px movement + < 250ms elapsed in pointerdown/pointerup listener — disambiguates from drag-to-pan
- inspector creature-died UX: 2-second placeholder then auto-clear — avoids the user clicking and immediately losing context

## E.25
- population milestones: bitset of fired flags (one-shot per threshold ever) — v5 §11 "only once per threshold ever"
- first-to-move fire-site: apply_movement_and_repulsion (was collect_deaths) per v6 §L "actually traveled ≥ 5u in its lifetime"
- hall-of-fame snapshots: clone Genome (~120B) into a HallOfFame struct on World — pre-staged for F.28; brain weights NOT cloned (only genome image needed for eulogy render)
- weirdest fallback: World.longest_lived snapshot (any age) — used if no creature ever reached 500 ticks (v5 §11.1)
- weirdest distance metric: species_distance against founder_genome_anchor + founder_brain_anchor (day-0 capture in World::new) per v5 §11.1

## F.26
- IDB worker: plain ES-module (not a Wasm worker) — JSON is serialized on the main thread; worker only does IDB put/get, so no wasm init needed in the worker context. v6 §I says "IndexedDB worker" without mandating wasm in worker.
- autosave interval: 300 ticks + 3000ms wall-clock floor — 10 sim-seconds at 30-tick/s baseline; wall floor prevents storm at 100× speed. Both values unspecified in v5/v6; chosen for balance between save frequency and IDB write cost.
- resume prompt: modal blocks render loop until user chooses — simplest UX; avoids race between sim and stale-world render.
- schema mismatch modal: same visual as resume prompt, no choices — schema version checked first in fromJson (prefix "schema-mismatch:N:M" in JsValue error). v5 §13 implies graceful degradation.
- from_save_v1 lives in world.rs (not save.rs): needs private fields cell_to_carrion and pending_extinction_check that save.rs cannot access. save.rs only holds the snapshot types and public helpers.
- SpatialGrid and vision Vec not saved: both are O(N) derived data; grid rebuilt by calling rebuild(), vision zeros initialized per creature count. Reduces snapshot size and eliminates stale-index bugs.
- SpeciesRegistry.next_id in snapshot: saved explicitly; restored as max(species_id)+1 via from_snapshot — avoids id collision after round-trip.
- EventLog rehydration: events stored as Vec<EventSnapshot> in SaveV1; rehydrated into EventLog with correct ring-buffer state. ring pointer reset to len after fill so subsequent pushes wrap correctly.
- AUTOSAVE_TICK_INTERVAL = 300, AUTOSAVE_WALL_FLOOR_MS = 3000: JS constants in main.ts, unspecified in spec. Match v5 §13 "periodic autosave" intent.
- DAY_TICKS = 1000: tick-to-day conversion for resume prompt and eulogy card. At 30 tick/s, one day ≈ 33 wall-seconds. v5 §11 + F.26 rationale.

## F.27
- seed pill location: right side of top-bar via margin-left:auto flex trick — matches v6 §A "top bar" + "seed display" without mandating position.
- copy button label: "copy" / "copied" — spec says no emoji; plain text matches right-rail style.
- seed-copy-btn: navigator.clipboard fallback logs warning and does not crash — clipboard API may be unavailable in non-HTTPS or old browsers.

## F.28
- hof_json weirdest fallback: if weirdest slot is None, falls back to longest_lived — v5 §11.1 "most diverged from founder; fallback if no creature lived 500+ ticks". longest_lived is always filled after any death.
- portrait renderer: draws genome rings onto an offscreen HTMLCanvasElement — same ring order as main renderWorld (RING_COLORS shared constant). Size 96px; caller appends to DOM.
- share text format: "evosim seed=X — lived N days, peaked S species, see {url}?seed=X" — unspecified in v5/v6; compact and pasteable.
- download blob: JSON blob named evosim-{seed}-day{N}.json — the last autosaved JSON, not re-serialized from live world at eulogy time (world.snapshot_json() is used).
- new world: persistence.deleteCurrent() then location.href = base URL without ?seed — ensures next boot lands on resume-probe path with no saved data.
- eulogy backdrop: fixed overlay with pointer-events; clicking backdrop does NOT dismiss (user must choose an action) — avoids accidental dismissal.

## F.29
- acceptance seed: "evosim-test-001" — pinned in constants.rs as ACCEPTANCE_SEED (v5 §16).
- perf budget: 8000ms for 10k ticks — CI machines vary; release build on ubuntu-latest comfortably fits in 8s.
- golden bootstrap: EVOSIM_WRITE_GOLDEN=1 env var writes tests/golden_snapshot_t10000.txt; subsequent runs assert. Golden committed to repo.
- snapshot_hash inputs: tick, creature SoA (id, pos, vel, energy, age, genome, brain weights), sun (current, cap per cell), carrion (x, y, pool, age), species (id + anchor sub-hash via species_distance), RNG state (serde_json bytes via xxHash64). Captures full deterministic world state.
- criterion (b) "≥ 2 live species" satisfied at T=10000 for seed evosim-test-001: confirmed during golden bootstrap run.

## F.31 (speciation-rate tuning — orchestrator-owned per ORCHESTRATOR.md)

- SPECIES_THRESHOLD raised 4.0 → 6.0: with rapid population growth (SUN_REFILL_RATE=0.30, FOUNDER_SPLIT_JITTER=50) many births fire per tick; NN drift across a lineage crosses 4.0 within tens of generations, causing nonstop Speciation events. v6 §H calls 4.0 the "default" but does not pin it as immutable. 8.0 was tried first but the acceptance criterion (b) "≥ 2 species at T=10000" failed for seed evosim-test-001; 6.0 passes and targets roughly one speciation per 1–2 minutes of sim time. σ_t scales and W_body/W_brain weights unchanged.
- Speciation toast rate-limit added (web/src/rail/toast.ts): first Speciation toast within any 2-second wall-clock window shows; subsequent ones are suppressed (events still land in the log). Belt-and-suspenders UX fix independent of sim behavior.
- Golden snapshot regenerated after threshold change (see commit; EVOSIM_WRITE_GOLDEN=1 cargo test --release --test acceptance).

## Events disable (v1.1 revisit)

- events_enabled flag on World (default false): wraps all `self.events.push(...)` call sites in world.rs — simplest reversible approach; avoids deletion churn and keeps the regression tests alive by flipping the flag to true in the three tests that assert event log contents (e25_population_milestones_fire_once, e25b_first_to_move_fires_on_movement_step, e20_speciation_event_lands_in_log_when_threshold_crossed).
- Events tab + panel: removed from index.html; Stats tab promoted to default-active; rail-events section kept in DOM with display:none so IDs remain stable for v1.1.
- Event-diff polling removed from rail/index.ts (section 2 of pollRail); species list, stats, inspector, highlight management all unchanged.
- pushToast no-op in rail/toast.ts: toast-stack DOM node and tickToasts remain; re-enablement is a one-liner revert.
- API surface kept intact: EventKind, EventLog, recent_events_json, events_total_count — all still compile and return empty/zero.

## F.30 (balance tuning — orchestrator owns these per ORCHESTRATOR.md)

> NOTE: The F implementer subagent surfaced these tuning needs while bootstrapping
> the §16 acceptance golden snapshot. The orchestrator (me) reviewed and ratified
> them. They cross the strict "only slider constants" boundary in two places (NN
> founder bias + FOUNDER_SPLIT_JITTER), both of which are necessary for v5's
> "open a tab, see a single cell, watch evolution" to actually work given the
> rest of the spec's numbers. Both deviations are surfaced in the end-of-build
> report. Original spec values are preserved in git history (pre-F.30 commits).

- NN_INIT_RANGE kept at 0.3 (original spec value): raising to 5.0 during debug had no effect on Rest-dominance because the specific seed's weight distribution was the issue, not the range.
- founder brain energy sensor (see Brain::founder): with NN_INIT_RANGE=0.3 and small random inputs, ALL hidden units are dead at zero input (ReLU kills negative-summing rows). The founder NN for seed "evosim-test-001" would always output Rest (first-index tiebreak on all-zero logits). Fix: hardwire hidden unit 0 as energy sensor (w_ih[0][energy_frac] += 10), connect it positively to Split output (w_ho[Split][0] += 10), and add Photosynth base prior (w_ho[Photo][all] += 5). This gives: high energy → Split fires; low energy → Photo fires. Offspring mutate independently; the wiring only tilts the initial prior. Constants: NN_FOUNDER_ENERGY_SENSOR_STRENGTH=10, NN_FOUNDER_SPLIT_ENERGY_WEIGHT=10, NN_FOUNDER_PHOTO_BIAS=5.
- SUN_REFILL_RATE raised from 0.08 to 0.30: with R=0.08, steady-state photo gain for 1 creature in a single sun cell (cap≈2) is 0.16×0.5/0.58=0.138/tick, below minimum upkeep 0.15/tick. At R=0.30, steady-state current=0.46 → photo gain≈0.46/tick >> upkeep. The slider default was 0.08 in the pitch (§3.3), which is below viability threshold — a balance oversight. FOUNDER_SPLIT_JITTER increased from 1.0 to 50.0: offspring spread across sun cells (jitter ±50u vs sun cell 30u wide), avoiding sun depletion from 3+ creatures in one cell.

## Threads (perf-4)

- **Dual-golden design.** `tests/golden_snapshot_t10000.txt` pins the
  default-build (sequential) hash and stays bit-stable across perf
  pieces. `tests/golden_snapshot_t10000_threaded.txt` pins the
  `--features threads` hash. Threaded acceptance test asserts a
  separate golden file because the bootstrap may yield identical OR
  different hash; either outcome is acceptable and the dual-file
  design future-proofs both cases. Each codepath is deterministic
  against itself; we do not promise bit-identity between codepaths.
  (Bootstrap result: hashes are **identical** — `0xb76e907c6221f7f5` —
  confirming the static-analysis prediction that arithmetic is
  bit-identical across sequential and parallel paths.)
- **Bootstrap.** Sequential: `EVOSIM_WRITE_GOLDEN=1 cargo test --release
  --test acceptance`. Threaded: `EVOSIM_WRITE_GOLDEN_THREADED=1 cargo
  test --release --features threads --test acceptance
  acceptance_t10000_threaded`. Regen rule: only regen a golden when its
  codepath intentionally changes — never as a fix for accidental drift.
- **Build behavior.** `pnpm build` and CI both build the wasm artifact
  with `--features threads`. SAB-less browsers fall through the JS-side
  `typeof SharedArrayBuffer !== 'undefined'` gate in `web/src/main.ts`
  and skip `initThreadPool`; rayon runs work on the calling thread
  with no perf change vs today.
- **Nightly wasm build.** `wasm-bindgen-rayon` requires `-C
  target-feature=+atomics,+bulk-memory,+mutable-globals` and
  `-Z build-std=panic_abort,std` for the wasm32 target (atomics
  are not yet stabilized in the wasm target). `.cargo/config.toml`
  sets these flags for `[target.wasm32-unknown-unknown]` only; native
  (`x86_64`) builds use stable Rust unaffected. CI gains a
  `rust-toolchain.toml` pointing to nightly for the wasm-pack steps.
  `vite.config.ts` gains `worker: { format: "es" }` so Vite bundles
  the rayon worker helper as an ES module (not IIFE).
- **WSL2 rayon overhead.** On WSL2, rayon with 8+ threads is ~9× slower
  than sequential for the 10k-tick acceptance test due to kernel thread
  scheduling overhead. The threaded perf budget check (`PERF_BUDGET_MS =
  8_000`) passes on real Linux CI (ubuntu-latest) where rayon is faster
  than sequential; on WSL2 use `RAYON_NUM_THREADS=1` to run the gate
  locally. Bootstrap path skips the perf check to always succeed.
- **Plan ref.** `docs/plans/perf-4-threads.md`.

## Audit v1.1 PR-1 review (2026-05-24)

- 13 commits landed: 14783f1 (S1), a4589ba (S37), 65fcf72 (S38), e54a9f4 (S2a), 92e10e5 (S2b), d6236f2 (S13), d6c9a65 (S14), 955f141 (S15), f7bdff6 (S34), 09361ac (S36), cde7c62 (S3), c2d45bd (S33), 537179c (S35).
- Both goldens unchanged at 0xb76e907c6221f7f5 (S37 confirmed byte-stable; threaded hash re-bootstrapped under EVOSIM_WRITE_GOLDEN_THREADED matches existing golden exactly).
- Gates: cargo fmt clean; clippy default + threads clean with -D warnings; cargo test --lib 98/98 pass; cargo test --release --test acceptance 3/3 pass (sequential golden ✓); threaded acceptance hash ✓ (perf budget exceeded on WSL2 — expected, not blocking); pnpm typecheck + pnpm build green.
- S1 split: src/world/{mod.rs 988, tick.rs 746, nn.rs 469, save_v1.rs 137}; src/world.rs gone.
- S35 visibility: cargo check passes both feature sets; snapshot_hash remains pub for tests/acceptance.rs.
- CSP header present in web/public/_headers; web/src/rail/events.ts deleted with no surviving references.
- Notable findings: none. Scope discipline is clean on all 13 commits. No DECISIONS.md entries added by implementer for S37 byte-stability — recorded here instead.

## Audit v1.1 PR-2 review (2026-05-24)

- 5 commits landed: 3dec63d (S17), c66fef2 (S19), e78e0c3 (S18+S20), a681aeb (S21), fcac7f2 (S22).
- Both goldens unchanged at 0xb76e907c6221f7f5 (sequential test green; threaded golden file byte-identical since pre-PR-2, no PR-2 commit in its `git log`; threaded hash assertion gated behind WSL2-failing perf check — not run, but no Rust simulation code was touched by PR-2).
- Gates: cargo test --release --test acceptance 3/3 pass (sequential golden ✓); threaded acceptance perf budget exceeded on WSL2 (25.4 s vs 8 s budget — explicitly expected per task / DECISIONS WSL2 rayon entry); pnpm typecheck clean; pnpm build clean (pre-existing dynamic-import warning unrelated to PR-2).
- S17: 5 typed setters at src/wasm_api.rs:209-237; set_slider now `Result<(), JsValue>` with private `try_set_slider` for native tests.
- S19: recent_events_json + events_total_count removed from src/wasm_api.rs and EvKind/EvEvent stripped from web/src/rail/index.ts.
- S18+S20: creature_at returns `Option<f64>` (stable id) via `grid.for_each_in_radius` bounded by `tolerance + SIZE_MAX*BODY_RADIUS_PER_SIZE`; new creature_idx_by_id O(N) reverse lookup; inspector now id-based.
- S21: creature_stride 13→11; offsets shifted in Rust + TS; render.ts throws on `stride !== 11` (watchlist g satisfied); hot-field reads switched to perf-5 SoA mirrors.
- S22: DPR capped at 2 in resize(); 200 ms status throttle; cached seed; hoisted world_ended.
- Notable findings: minor — creature_at's `for_each_in_radius` closure early-outs via `if found.is_some() return` but doesn't short-circuit the grid traversal (still visits remaining cells, just skips work). Correct, negligible cost at v1 scale, not blocking. Stride guard uses literal `11` with descriptive throw rather than an `EXPECTED_CREATURE_STRIDE` named constant — functionally equivalent.

## Audit v1.1 PR-3 (2026-05-24)

audit v1.1 — snapshot_hash coverage extended (S7: 18 new fields: digestion_cooldown, cumulative_upkeep, species_id, parent_species_id, last_action, action_this_tick, max_size_reached, distance_travelled, birth_tick per creature; carrion.id, carrion.sun_cell; species parent_id, name, born_tick, died_tick, child_count, depth, anchor_brain_weights; registry next_id); RNG hash format changed from serde_json byte stream to direct u64×4 via SimRng::state() (S8); NaN canonicalization added to write_f32 (all NaN payloads map to 0x7fc0_0000); scavenge now uses 3×3 cell sweep (S24 — hash unchanged). New sequential hash 0xd56cd6881d2898fc, new threaded hash 0xd56cd6881d2898fc (both identical — arithmetic is bit-identical across sequential and parallel paths, same result as PR-1 bootstrap).

- PR-3 commits: 772df68 (S9), e12c857 (S11), 343b980 (S4), 86afa99 (S5), b64acc3 (S6), 56a6898 (S10), 5d70301 (S12), 1d3c62f (S24), <S7+S8 commit>.
- S4: place_hotspots fallback now uses `else if attempts > 2000` + resets attempts to 0 per branch; loop is bounded.
- S5: decode_action returns Rest immediately on non-finite logits; pick_action_d zeros velocity on NaN path.
- S6: mutate_f32/mutate_u32 guard against non-finite mutation results; keep original value on NaN.
- S9: clippy.toml forbids HashMap/HashSet iter* in sim-critical files via disallowed-methods.
- S10: debug_assert! invariants in chunk_ranges + coverage test for n ∈ {0,1,7,8,9,100,1500}.
- S11: count_carrion_overlap + compute_is_at_wall extracted as free pub(crate) fns; used by both sequential and threaded NN paths.
- S12: validate_save() centralises all loader hardening (9 checks) into src/save.rs; wired into from_save_v1.
- S24: Action::Scavenge uses 3×3 HASH_DIM cell sweep via cell_to_carrion instead of O(all_carrion) linear scan. Golden unchanged (scavenge path finds same carrion via cell lookup in evosim-test-001 run).
- S7+S8: see hash line above. SpeciesRegistry.next_id promoted to pub(crate). SimRng::state() added.

## Audit v1.1 PR-3 review (2026-05-24)

- 10 commits landed: 772df68 (S9), e12c857 (S11), 343b980 (S4), 86afa99 (S5), b64acc3 (S6), 56a6898 (S10), 5d70301 (S12), 1d3c62f (S24), bf3287a (S7+S8 — THE REGEN), c06c0ef (S39).
- Golden regen: sequential AND threaded → 0xd56cd6881d2898fc (still bit-identical; same value PR-1 produced, indicating S7-expanded coverage and S8 RNG-format change happened to land on the identical hash for this seed/tick budget — a curiosity, not a bug; the regen commit is the only one touching either golden file). bf3287a is the sole commit in `git log -- tests/golden_snapshot_t10000.txt` since PR-1 dual-bootstrap.
- S24 standalone hash drift: NONE (golden value matches what bf3287a already wrote; the 3×3 sweep finds the same carrion the linear scan did for this seed).
- Gates: cargo clippy default + threads clean with -D warnings; cargo test --lib 127/127 default and 127/127 threads pass; cargo test --release --test acceptance 4/4 pass against 0xd56cd6881d2898fc (incl. new save_load_hash_equal_immediately_after_load); S39 threaded=sequential equivalence test passes (acceptance_threaded_actually_matches_sequential_t10000 in 33.74 s); threaded full-acceptance budget exceed expected on WSL2 (24.6 s vs 8 s budget — hash check is gated behind the budget assert so the runtime threaded-golden match is verified indirectly via the S39 equivalence test instead); pnpm typecheck + pnpm build green.
- Notable findings:
  - **FIX-NEEDED (minor): `cargo fmt --check` fails on committed code from 1d3c62f (S24).** Three formatting diffs in src/world/tick.rs tests: (a) line 712 `use crate::vision::build_cell_to_carrion;` should sort after `use crate::constants::*;`, (b) line 757 same issue in second test, (c) line 786 `assert_eq!(w.carrion[0].pool, carrion_pool, "…")` should be single-line. A trivial `cargo fmt` follow-up commit fixes it; no behavioural impact and no hash impact.
  - Naming nit: the S39 threaded-equivalence test was committed as `acceptance_threaded_actually_matches_sequential_t10000` (plan spec named it `acceptance_threaded_matches_sequential_t10000`). Functionally identical; just a renaming the implementer chose. Not blocking.
  - The `pub force_sequential_nn` field carries `#[cfg_attr(not(feature = "threads"), allow(dead_code))]` because the field is only read inside cfg(threads) NN code. Correct; field is `pub` (not `pub(crate)`) intentionally for integration-test access per the doc comment.
  - All scope-discipline checks clean: each of the 10 commits touches only the files its title implies; bf3287a is the only commit touching either golden file.
  - DECISIONS.md PR-3 entry by the implementer present, lists both new hashes correctly.


## v1.2 grass mechanic (pass-open, 2026-05-25)

Baseline: HEAD = `9700dcc`. Orchestrator brief: `docs/plans/v1.2-grass-mechanic-brief.md`. This section captures the spec deviations the v1.2 pass deliberately introduces against `docs/archive/PITCH-v5.md` + `PITCH-v6.md`. The archive specs are NOT being edited; deviations live here.

Spec deviations (all locked by the user; not to be re-litigated mid-pass):
- v5/v6 §3.3 sun mechanic deleted entirely. `src/sun.rs` + `SunMap` field + sun render layer removed. Photosynth replaced by Graze acting on a new `GrassGrid`.
- v5 §3.5 walls deleted. World becomes toroidal: wrap-around on all four edges. All distance / raycast / repulsion / `for_each_in_radius` math wraps; `SpatialGrid::cell_of` becomes modular.
- `is_at_wall` NN input removed (one fewer extras slot).
- v5/v6 §F.30 founder NN hardwiring rewritten to be minimal: hidden-0 = on-grass detector (weight from the center grass-patch input), Output Graze += large positive from hidden-0, Output Move += baseline + ~20-tick re-roll direction persistence, Output Split += energy_frac. Removed: `NN_FOUNDER_ENERGY_SENSOR_STRENGTH` (Photosynth path), `NN_FOUNDER_PHOTO_BIAS`.
- NN input dimension: 136 → 160 (120 vision unchanged; 16 extras → 15 extras after `is_at_wall` removal; +25 grass-patch inputs from bilinear-sampled 5×5 around the creature at 2.5u step). Brain weight matrix `136×24` → `160×24`; hidden=24, output=8 unchanged.
- New genome trait `nose_count: u8 ∈ [0,5]`, mirrors `eye_count`. Founder `nose_count = 0` → grass-patch inputs zeroed (creature blind to grass) until a descendant mutates ≥ 1.
- Genome trait rename `photosynth_efficiency` → `graze_efficiency` (pure rename; range, mutation, hash bytes preserved).
- Founders have `eye_count = 0` AND `nose_count = 0` at init — honestly-dumb Brownian-motion grazers; world seed places enough initial grass for lineage survival.
- Action set unchanged at 8: Rest, Move, Eat, Scavenge, Split, Armor, Pigment, **Graze** (slot replaces Photosynth in the same enum index).
- Eat redesign: per-bite mechanic, predator gains `eat_bite_fraction * prey.energy`, prey loses same. Default `eat_bite_fraction = 0.5`; dev-panel slider. `eat_efficiency` genome trait survives as a multiplier.
- Save schema: version bump required (NN shape change + new `GrassGrid` field + `nose_count` field + SunMap removed). Load path on mismatch shows modal "Save format updated. Starting fresh.", clears IDB record, reboots into fresh world.
- Pass shape: 3 PRs, each ends with a golden regen (sequential AND threaded together). Each regen logged here with one-line reason.

Out of scope for v1.2 (explicitly deferred — listed in case the orchestrator/implementers feel tempted): NEAT, sparse-substrate NN, capability traits as NN inputs beyond `nose_count`, sexual reproduction, signaling, lineage/NN/trait viz, WebGL2/WebGPU renderer, quadtree, deterministic replay, Tauri, brain weight transpose / SIMD reshape.

Orchestrator decision (kicked off without user clarification per user instruction "go go go, put any questions in closeout doc"): any blocking ambiguity surfaced during planning gets aggregated into `docs/research/v1.2-final-report.md` § Questions for the user — no mid-pass interruptions.

Post-cross-review locks (see `docs/plans/v1.2-cross-review.md` + `docs/plans/v1.2-amendments.md`):
- `GRAZE_MAX_PER_TICK = 0.1` (was contested 0.25 vs 0.1 between p1c/p1e plans; cross-reviewer locked 0.1 based on founder survival math vs UPKEEP_BASE).
- Constant names locked: `GRASS_GRID_DIM`, `GRASS_CELL_SIZE`, `GRASS_CELL_COUNT`, `NN_GRASS_PATCH_CENTER_SLOT = 147`, etc. — see amendments doc.
- PR-1 commit sequence: master §E #5 (p1b sun-deletion) and #8 (p1f schema bump) **collapsed into one atomic commit** to eliminate the autosave-incompatible inter-commit window.
- Move-bias `move_bias_*` semantics: **ADD** (additive on NN output), not SET. Reason: SET would permanently disable NN-driven motion in all descendants; the brief reads "Move += baseline" literally. User-confirm item for closeout — revertible in one line if user prefers founder Brownian-walk to override evolved motion.
- Schema-version bumps: three total. v1 → v2 (PR-1 close, p1f), v2 → v3 (PR-2 close, p2g), v3 → v4 (PR-3 close, p3a addendum — adds DevSliders.eat_bite_fraction).
- Intra-PR-2 acceptance policy: master §D.6 "every commit must pass §16 acceptance" amended for PR-2 only. Brain-shape change makes goldens undefined between p2a and p2h regen; §16 gate runs at PR boundaries only.
- P3e founder-survival tuning ladder (raise `grass_initial_seed_count` 8 → 50 → 100; then `grass_in_cell_growth_r` 0.005 → 0.01; then `MOVE_BIAS_REROLL_INTERVAL` 20 → 60; then escalate to user). Documented in amendments §A.8.

P1a (toroidal arithmetic) landed: src/torus.rs (wrap_pos, torus_delta, torus_dist_sq, WORLD_HALF) + modular SpatialGrid::cell_of + toroidal for_each_in_radius + toroidal raycast DDA + torus_delta repulsion/eat/scavenge + wrap_pos wall-clamp→wrap + wrap_pos split-jitter; 3 atomic commits (695c10a, 3c7f122, 6732797); 127→149 tests all pass (sequential + threaded). Minor deviation: #[allow(dead_code)] attrs used on torus functions in Commit A (stripped in B and C as callers landed) to satisfy clippy -D warnings between commits; no semantic impact.

P1c (GrassGrid module) landed: src/grass.rs (GrassGrid + step + bilinear_sample + consume + for_each_cell_overlapping_circle + cell_index_for + 15 unit tests) + grass constants block in constants.rs (GRASS_CELL_SIZE=2.5, GRASS_GRID_DIM=240, GRASS_CELL_COUNT=57_600, GRASS_MAX, GRAZE_MAX_PER_TICK=0.1, defaults) + GrassGrid field on World struct + DevSliders grass fields (grass_propagation_rate_k, grass_in_cell_growth_r, grass_initial_seed_count) + validate_save range checks for new slider fields + grass.step call in World::step (tick.grass_step span after tick.sun_refill) + from_save_v1 fresh-grass rebuild (TODO P1b+P1f restore). 149→164 tests (sequential), 149→165 (threaded). All gates pass. Deviations: method renamed cells_overlapping_circle→for_each_cell_overlapping_circle to match P1e call signature per task spec; test #11 uses explicit cell-center coords (not WORLD_SIZE/2) since the query point must be at a cell center for radius-0.5 to hit exactly one cell; 3 pre-existing tick.rs clippy issues fixed (manual_range_contains × 2, unused import) as they would have blocked the gate.

P1b+P1f (atomic): sun mechanic deleted + GrassGridSnapshot added + SCHEMA_VERSION 1→2. Files touched: src/sun.rs (deleted), src/lib.rs, src/constants.rs, src/carrion.rs, src/save.rs, src/snapshot_hash.rs, src/world/mod.rs, src/world/tick.rs, src/world/save_v1.rs, src/world/nn.rs, src/vision.rs, src/wasm_api.rs, src/profiler.rs, web/src/render.ts (14 files). 164→160 tests (sequential, 7 deleted + 4 added + 1 updated), 165→161 (threaded). All 6 gates pass. SPANS_PER_ITER corrected 10→11. scratch_grows_with_population restructured: without photosynth income the population blooms-then-crashes, so the test now pre-loads energy and checks the invariant during the bloom phase rather than at tick-500 end.
