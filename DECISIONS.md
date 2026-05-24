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

