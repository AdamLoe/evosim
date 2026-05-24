# Cross-piece review — v1.1 perf + UI pass

**Reviewer scope.** Read all 8 detailed plans plus the master
(`perf+ui-master.md`) and inspected the surrounding source for the
overlap-prone files (`src/world.rs`, `src/wasm_api.rs`,
`web/src/rail/stats.ts`, `web/index.html`, `web/src/styles.css`). The
brief from §6 of the master plus the orchestrator's explicit checklist
drove the search. Verdict per pair below, then must-fix list, then
commit-ordering recommendation, then integration timeline.

---

## 1. Per-pair verdicts

### A — perf-5 SoA mirror vs ui-inspector / `creature_inspect_json`

**Verdict: NO CONFLICT.** Both plans pin the same contract:

- perf-5 §"What is intentionally NOT in this plan" and §"Critical
  rule" both state `genomes: Vec<Genome>` stays; mirrors are
  **additive**.
- perf-5 §D2 + §"Files & function signatures / src/wasm_api.rs" =
  **No changes** — inspector continues to read AoS.
- ui-inspector §7 "Stable contracts" table lists
  `creature_inspect_json` JSON shape under
  `src/wasm_api.rs:243` as **untouched**, and `renderInspector` (15
  `#ins-*` writes) needs zero edits.

The two plans agree the inspector continues to see all 14 genome
fields including the cold ones (`armor`, `max_age`, `pigment_*`,
`bite_reach`, `mutation_rates`) via AoS.

### B — perf-4 threads vs wasm-bindgen export surface

**Verdict: NO CONFLICT.** perf-4 §D1 only **adds** `init_thread_pool`
as a new free-function export. The JS-visible signatures of
`WorldHandle::step` (src/wasm_api.rs:47), `step_n` (:53),
`population()` (:73), `creatures_buffer`, `creature_at`,
`creature_inspect_json`, `species_list_json`,
`profile_enable`/`profile_report_json` are all untouched. perf-4 §R8
explicitly verifies the new export does not collide with
`WorldHandle`.

### C — ui-perf vs `profile_report_json` JSON shape (and perf-4)

**Verdict: NO CONFLICT.** ui-perf §D9 pins JSON contract as
unchanged; perf-4 §3 confirms profiler untouched ("the profiler
spans/structure are unchanged"). Both Rust-side
(`profile_report_json`) and TS-side (`reportJson` from
`web/src/perf.ts`) APIs survive verbatim.

### D — Four perf plans co-editing `src/world.rs`

**Verdict: MANAGEABLE WITH ORDERING — one real overlap that needs
explicit rebase note.** Inventory by edit region:

- **perf-1** (sector trig): no `world.rs` edits (per perf-1
  §"Files & function signatures / src/world.rs" → "**No changes
  required.**"). Founder/birth/save-restore go through
  `CreatureSoA::push`. Constant relocation (`SECTORS`,
  `EYE_STRIDE` → `constants.rs`) doesn't touch world.rs either.
- **perf-2** (scratch pool): edits `World` struct fields (line ~101),
  `World::new` initializer (~132–165), `from_save_v1` (~1076–1110),
  `apply_movement_and_repulsion` lines 480–525 (fx/fy/neighbors) and
  528–556 (write-back), `eat_and_scavenge` lines 611–718.
- **perf-3** (grid cursor): zero `world.rs` edits. Self-contained in
  `src/grid.rs`. Confirmed.
- **perf-5** (genome SoA): rewrites read sites in
  `photosynth_two_pass` (561–604), `apply_movement_and_repulsion`
  (lines 485, 496, 531), `eat_and_scavenge` (625–688),
  `energy_bookkeeping` (721–781), `count_carrion_overlap`/
  `compute_is_at_wall` (1117–1161), `build_nn_input`/`pick_action_d`
  (1207–1306), and the threaded NN closure at line 374.
- **perf-4** (threads): adds an `init_thread_pool` re-export in
  `src/lib.rs` (not world.rs); modifies `src/vision.rs` only for the
  parallel `run`. **No `world.rs` edits in perf-4.** (Per perf-4 §3
  "Vision call-site changes. None." and §D6 "No `pub` upgrade on
  `chunk_ranges`.") The doc-comment update on `chunk_ranges` per R5
  is a 2-line comment-only change.

**Pairwise overlaps inside `world.rs`:**

| pair | functions touched by both | resolution |
|---|---|---|
| perf-2 ∩ perf-5 | `apply_movement_and_repulsion` (perf-2 promotes fx/fy/neighbors; perf-5 rewrites `genomes[i].size` reads at 485, 496, 531). `eat_and_scavenge` (perf-2 promotes 6 scratch vecs; perf-5 splits `g_i` bindings into mirror + cold AoS reads). | perf-2 lands first per master §4. perf-5 rebases on top — the changes are textually disjoint within each function (perf-2 changes the local `vec![]` lines and the `fx[i]` accessors; perf-5 changes `self.creatures.genomes[i].size` reads). No conceptual conflict. **Add explicit note to perf-5 implementer: "rebase against perf-2's `self.scratch_*` accessors before rewriting read sites; the size reads in apply_movement_and_repulsion are still `self.creatures.genomes[j].size`, not on the scratch arrays."** |
| perf-2 ∩ perf-4 | none — perf-4 doesn't touch world.rs. | — |
| perf-5 ∩ perf-4 | **Site G in perf-5 §"world.rs:351–424"** rewrites `creatures_ref.genomes[i].size` at line 374 inside the existing `#[cfg(feature="threads")]` block. perf-4 doesn't touch that block. | Per master §4, perf-5 lands BEFORE perf-4. So perf-5 modifies the existing threaded NN block in place; perf-4 then doesn't touch line 374. Clean. **However:** perf-4 adds `--features threads` to CI. If perf-5 lands first WITHOUT the threaded CI, perf-5's Site G rewrite (`creatures_ref.g_size[i]`) is **compiled but not tested in CI** until perf-4 lands. Catch only happens on the local `cargo clippy --features threads` step in perf-5 §"Sequencing #5" — implementer must run it. **Confirmed: perf-5 §"Sequencing" step 5 says "Threaded acceptance. (Only if the threaded golden from perf-4 exists yet.)"** — phrasing is correct but the implementer should still run `cargo clippy --all-targets --features threads -- -D warnings` even before the threaded golden exists. |
| perf-1 ∩ perf-2/-3/-5 | none (perf-1 doesn't touch world.rs). | — |

### E — `CreatureSoA::push` collision (perf-1 + perf-5)

**Verdict: NO CONFLICT BY DESIGN, but commit ordering matters.** Both
plans add lines to `CreatureSoA::push`:

- perf-1 adds: `self.eye_trig.resize(new_len, 0.0); self.recompute_eye_trig_at(i);`
  AFTER the existing `genomes.push` / `brains.push`.
- perf-5 adds: `self.push_hot_mirrors(&genome);` BEFORE
  `self.genomes.push(genome);` (to avoid the move-then-borrow tangle).

Per master §4 ordering: perf-1 → perf-2 → perf-3 → perf-5 → perf-4.
perf-5 lands AFTER perf-1, so the implementer of perf-5 sees:

```rust
// after perf-1:
pub fn push(...) -> usize {
    // ... existing pushes ...
    self.genomes.push(genome);
    self.brains.push(brain);
    let new_len = self.eye_trig.len() + SECTORS * 2;
    self.eye_trig.resize(new_len, 0.0);
    let i = self.x.len() - 1;
    self.recompute_eye_trig_at(i);
    i
}
```

perf-5's `push_hot_mirrors(&genome)` MUST run BEFORE
`self.genomes.push(genome)` because it borrows `&genome`. The
resulting merged shape is:

```rust
pub fn push(..., genome: Genome, brain: Brain) -> usize {
    // ... existing primitive pushes ...
    self.push_hot_mirrors(&genome);        // perf-5
    self.genomes.push(genome);
    self.brains.push(brain);
    let new_len = self.eye_trig.len() + SECTORS * 2;  // perf-1
    self.eye_trig.resize(new_len, 0.0);
    let i = self.x.len() - 1;
    self.recompute_eye_trig_at(i);          // perf-1
    i
}
```

**Action: must-fix M1 below adds an explicit cross-piece note to
perf-5 about merging with perf-1's edit.** Same applies to
`with_capacity` (both add lines) and `remove_indices` (perf-1 adds
`swap_remove_chunk(&mut self.eye_trig, k, SECTORS*2)`; perf-5 adds
seven `swap_remove(k)` calls). All disjoint — just two layers of
"append to the existing loop body" edits.

### F — `web/src/rail/stats.ts` collision (ui-stats + ui-perf)

**Verdict: CLEAN by design but worth pinning ordering.** Plans agree:

- ui-stats §4a: "**no changes in this commit**" to `rail/stats.ts`.
  Lines 1–99 (charts) stay; lines 101–270 (profiler) are
  ui-perf's territory.
- ui-perf §"TS module migration / Removals in
  `web/src/rail/stats.ts`": delete lines 101–270 + the line-6
  `setProfilerEnabled / isProfilerEnabled / reportJson` import.

Disjoint contiguous ranges = trivial rebase regardless of order. Per
both ui-stats §1 D4 and ui-perf §C1, the file ends up containing
only charts; ui-stats §1 D4 explicitly says "ui-perf does NOT also
rename this file" so neither piece moves it to `widgets/stats.ts`.
**Confirmed: file rename is out of scope for both pieces.**

### G — `.overlay-widget` CSS class ownership

**Verdict: CLEAN.** ui-stats §3a is the sole DEFINER of
`.overlay-widget` (with concrete values). ui-inspector §3.1 and
ui-perf §"CSS" both explicitly state "owned by ui-stats; this piece
consumes it" with fallback notes if ui-stats hasn't landed yet.
**No duplicate definition** if ui-stats lands first (the recommended
order).

**Caveat — fallback definitions could collide:**
- ui-inspector §3.1 provides a fallback `.overlay-widget` block with
  values slightly different from ui-stats's canonical: ui-inspector
  uses `pointer-events: auto; box-shadow: 0 2px 6px rgba(0,0,0,0.35);`
  whereas ui-stats uses `pointer-events: none;` (with child opt-in)
  and no box-shadow.
- ui-perf §"Dependency note" describes a minimal fallback in prose
  ("`background: var(--rail-bg); border: 1px solid
  rgba(255,255,255,0.08); border-radius: 4px; padding: 6px 8px;`")
  with no `pointer-events` — but its own `#perf-box` rule sets
  `pointer-events: none` on the container.

If ui-stats DOES land first (per master §4), neither fallback is
used and the canonical definition wins. **If ordering is honored,
this is a non-issue. Must-fix M2 below pins the ordering.**

### H — DOM ID stability union

**Verdict: CLEAN.** Union of preserved IDs across all UI plans:

| ID | preserved by | notes |
|---|---|---|
| `#aquarium` | all three UI plans | unchanged |
| `#top-bar`, `#status`, `#seed-display`, `#seed-value`, `#seed-copy-btn`, `#save-indicator`, `#toast-stack` | ui-stats §9 | global, untouched |
| `#right-rail` (hidden) | ui-stats §3b | `display: none` |
| `#rail-tabs`, `#rail-events`, `#rail-stats`, `#rail-inspector` | ui-stats §9, ui-inspector §2.2 | kept as empty/legacy sections |
| `#chart-pop`, `#chart-species` | ui-stats §2, §9 | moved into `#stats-box` |
| `#profiler-enable`, `#profiler-stabilizing`, `#profiler-table`, `#profiler-tbody` | ui-perf §"Stable contracts" | moved into `#perf-box` |
| all 15 `#ins-*` (species, parent, action, age, energy, size, photo, eat, scav, move, eyes, vision, armor, bite, pigment) | ui-inspector §2.4 | moved into `#inspector-box` |
| **NEW** `#stats-box` | ui-stats | new |
| **NEW** `#inspector-box`, `#inspector-close` | ui-inspector | new |
| **NEW** `#perf-box` | ui-perf | new |
| **REMOVED** `#profiler-panel` | ui-perf §D6 | no TS readers; verified via grep |

No plan removes a preserved ID. ui-perf removes `#profiler-panel`
which has zero TS readers (ui-perf §D6 verified via `grep -rn
"profiler-panel" web/`).

### I — Inspector tab injection vs hidden rail

**Verdict: CONSISTENT.** ui-inspector §D2 + §4.1 Edit A define
`isRailHidden()` and guard the legacy `ensureInspectorTab` /
`switchTab` calls. ui-stats §4b + R-S4 (Risk box) explicitly says
"the inspector tab-injection code runs (no console error); the
inspector data is updated in the hidden DOM but not visible." and
flags this as expected pre-ui-inspector state.

After ui-inspector lands, the tab injection no-ops behind
`isRailHidden()`. Consistent.

### J — Polling cadence collisions

**Verdict: CLEAN.** Three polling paths post-restructure:

1. **Stats chart sample** — stays inside `pollRail()` via
   `maybeSampleStats(world)` (ui-stats §1 D5). Runs every frame from
   `main.ts` RAF. Cost: one `world.stats_sample()` call per
   `SAMPLE_INTERVAL_TICKS`, plus chart redraw.
2. **Inspector refresh** — stays inside `pollRail()` via
   `refreshInspector(world, idsBuffer, rail)`. ui-inspector §D3 +
   Edit D adds an early-return when `#inspector-box` is
   `display: none`. When closed: zero wasm calls.
3. **Profiler 1Hz poll** — stays as its own `setInterval(1000)`
   inside ui-perf's `startPolling` (ui-perf §D8). Only runs when
   profiler checkbox is checked.

All three are independent. No timer collision. The 1Hz species poll
in `pollRail` (mentioned in master §6 R3) is preserved unchanged.

### K — Golden-snapshot bundle integrity

**Verdict: CLEAN.** No plan invalidates the existing
`tests/golden_snapshot_t10000.txt`:

- perf-1 §D6, §"src/save.rs", §"src/snapshot_hash.rs" — no save/hash
  edits, no test changes.
- perf-2 §6c — "No edits." to `tests/acceptance.rs`.
- perf-3 §4 — no save/hash edits; §6 acceptance unchanged.
- perf-5 §"src/save.rs", §"src/snapshot_hash.rs" — no edits.
- perf-4 §"tests/golden_snapshot_t10000.txt: unchanged" + dual-golden
  protocol explicitly preserves the sequential golden.

### L — Acceptance test scaffold

**Verdict: CLEAN with one nuance.** perf-4 §R3 Fix A applies
`#[cfg(not(feature = "threads"))]` to the three existing tests
(`acceptance_t10000`, `profile_does_not_change_hash`,
`save_load_step_preserves_determinism`) AND appends a new
`acceptance_t10000_threaded`. This is the only acceptance-test
scaffold edit. perf-1 through perf-5 (except perf-4) explicitly do
not touch `tests/acceptance.rs`.

**Nuance:** perf-5 §"Tests / T3" describes
`save_round_trip_rebuilds_hot_mirrors` as a `world.rs::tests` unit
test (not an acceptance test). Confirmed — it lives in
`src/creature.rs::tests` or `src/world.rs::tests`, not
`tests/acceptance.rs`. Clean.

### M — Constants & naming

**Verdict: CLEAN.**

- perf-1 §"src/constants.rs" — no new constants. Moves `SECTORS`,
  `EYE_STRIDE` from `vision.rs` to `constants.rs` and re-exports
  back for backcompat.
- perf-2 §1.3 — no `MAX_POPULATION` constant added (out of scope).
- perf-3 §1 D6 — no new constants.
- perf-4 §"Build infra" / §1 — uses existing `N_CHUNKS = 8` from
  `constants.rs:144`.
- perf-5 §"src/constants.rs" — no new constants.
- UI plans — no Rust constants.

No naming collisions on `World` fields:
- perf-2 scratch fields: `scratch_fx`, `scratch_fy`,
  `scratch_neighbors`, `scratch_damage`, `scratch_gain`,
  `scratch_cooldown_set`, `scratch_attempted_eat`,
  `scratch_attempted_scavenge`, `scratch_got_a_bite`.
- perf-1 SoA field on `CreatureSoA` (not `World`): `eye_trig`.
- perf-5 SoA fields on `CreatureSoA` (not `World`): `g_size`,
  `g_photo_eff`, `g_eat_eff`, `g_scav_eff`, `g_move_speed`,
  `g_vision_range`, `g_eye_count`.
- perf-3 field on `SpatialGrid` (not `World`): `cursors`.

Three distinct namespaces (`World`, `CreatureSoA`, `SpatialGrid`),
all collision-free.

### N — DECISIONS.md entries

**Verdict: PARTIAL — some plans should add entries but currently
don't.**

- perf-1 §"Sequencing #10" — explicitly **no DECISIONS entry**
  ("no spec or contract change"). Acceptable; perf-1 is purely
  internal.
- perf-2 §0 — no DECISIONS entry mentioned. Acceptable; internal.
- perf-3 §7 — explicitly no DECISIONS entry. Acceptable.
- perf-5 §"Sequencing #9" — no DECISIONS entry. Acceptable; v6 §M
  hash contract is unchanged.
- perf-4 §2h — REQUIRES new `## Threads (perf-4)` section. Pinned.
- ui-stats — no DECISIONS entry mentioned.
- ui-inspector — no DECISIONS entry mentioned (D-prefixed decisions
  are internal to the plan).
- ui-perf §"Decisions to record" — DOES specify DECISIONS entries
  for D1, D3, D4, D5, D6 under a `## v1.1 UI — ui-perf widget`
  heading.

Asymmetry: ui-perf wants to add DECISIONS entries, ui-stats and
ui-inspector don't. **Recommendation: either both ui-stats and
ui-inspector add brief entries (for symmetry / future
re-enablement reference) OR ui-perf drops its DECISIONS entries
(internal). Either is consistent; the current state is asymmetric
but not broken.** Flagged as should-fix S1.

### O — Single owner of `#right-rail { display: none }`

**Verdict: CLEAN.** ui-stats §3b is the sole owner. ui-inspector
§2.3 explicitly defers: "ui-stats's CSS rule owns that (the other
widget plans also rely on it). ui-inspector does not duplicate the
rule." ui-perf §HTML notes (last bullet) explicitly defers: "ui-perf
does NOT take responsibility for hiding the rail." Single rule,
single home.

---

## 2. Must-fix list

**M1. perf-5 must explicitly cite the perf-1 merge in
`CreatureSoA::push` / `with_capacity` / `remove_indices`.** Current
perf-5 plan §"Files & function signatures / src/creature.rs"
describes the edit as if no perf-1 changes exist. The implementer
of perf-5 (landing AFTER perf-1 per master §4) will see perf-1's
`eye_trig` field, `recompute_eye_trig_at` helper, and
`swap_remove_chunk` already in `push` / `with_capacity` /
`remove_indices`. perf-5's plan needs a one-paragraph "Merge note:
this plan was authored against pre-perf-1 source; after perf-1
lands, `CreatureSoA::push` already has the `eye_trig` lines —
insert `push_hot_mirrors(&genome)` BEFORE `self.genomes.push(genome)`
and the perf-1 lines stay after. Same shape for `with_capacity` and
`remove_indices`." Without this note, the perf-5 implementer might
mistakenly delete perf-1's additions.

**M2. perf-5 must add explicit rebase note for the `world.rs`
overlap with perf-2.** perf-5 rewrites
`apply_movement_and_repulsion` lines 485 (`ri`), 496 (`rj`), 531
(`r`) and `eat_and_scavenge` lines 625–688. perf-2 (landing first
per §4) will have already rewritten `fx`/`fy`/`neighbors` and the
six eat/scavenge scratch Vecs at OTHER lines in the same functions.
The perf-5 plan currently quotes pre-perf-2 line numbers and code
shapes. Add a paragraph: "Line numbers in this plan are pre-perf-2;
after perf-2 lands, the `fx[i]` and `gain[i]` writes become
`self.scratch_fx[i]` and `self.scratch_gain[i]`. perf-5's
`g.size`/`g.eat_efficiency`/etc. reads still apply but at slightly
shifted line numbers. The `for j in neighbors` loop is now `for &j
in &neighbors` per perf-2's `mem::take` recipe — perf-5 must not
re-write that loop header."

**M3. perf-4 should warn perf-5 implementer to validate threaded
clippy.** perf-5 lands BEFORE perf-4 per master §4. perf-5 Site G
edits the `#[cfg(feature="threads")]` block at world.rs:374. If the
perf-5 implementer skips `cargo clippy --features threads`, a
typo in Site G silently slips through (the default-build clippy
won't see it). perf-5 §"Sequencing" step 5 mentions threaded
acceptance only "if the threaded golden has landed" — but
**clippy --features threads** doesn't need the threaded golden.
**Add to perf-5 §"Sequencing" between steps 4 and 5: "Run `cargo
clippy --all-targets --features threads -- -D warnings` to validate
the Site G rewrite (which lives in the cfg=threads branch). This
runs even before perf-4 lands."**

**M4. ui-inspector's `.overlay-widget` fallback (§3.1) diverges
from ui-stats's canonical definition.** ui-stats §3a sets
`pointer-events: none` on `.overlay-widget` with child opt-in,
matching the `#top-bar` pattern. ui-inspector's fallback sets
`pointer-events: auto` and adds a `box-shadow`. If ui-stats lands
first (recommended), ui-inspector's fallback is dead code — but
ui-inspector then ALSO needs to override `pointer-events: none →
auto` on `#inspector-box` specifically (the inspector IS
interactive; the canvas-pan-through-the-box behavior that ui-stats
wants is the wrong default for the inspector). **Fix: either ui-stats
needs to make the rule `pointer-events: none` only for `#stats-box`
(not all `.overlay-widget` containers), OR ui-inspector adds a
`#inspector-box { pointer-events: auto; }` override after consuming
the base.** Either resolution is fine; just needs to be pinned.

**M5. ui-perf likewise sets `pointer-events: none` on `#perf-box`
(§D10 + CSS §"#perf-box { pointer-events: none }")**, which is the
correct outer-container behavior. Combined with M4: confirm the
final pattern is "outer container `pointer-events: none`, children
`auto`" applied uniformly via `.overlay-widget`, AND
ui-inspector explicitly overrides for the inspector
(`pointer-events: auto` on container) because the inspector box is
opaque and meant to capture clicks. The current plans don't
cross-reference this. **Action: ui-stats and ui-inspector must
agree on the pointer-events contract. Recommended resolution:
`.overlay-widget` defaults to `pointer-events: auto`; only ui-stats
explicitly sets `pointer-events: none` on `#stats-box` (since stats
needs the canvas-pan-through behavior); ui-perf sets `pointer-events:
none` on `#perf-box` (same reason); ui-inspector inherits the
`auto` default. This inverts ui-stats's current `.overlay-widget`
rule.**

---

## 3. Should-fix items

**S1. DECISIONS.md entry symmetry (N above).** Either all three UI
plans add brief entries or none do. Currently only ui-perf wants to.
Implementer's call.

**S2. perf-4 §R3 Fix A choice should be pinned in the plan rather
than left as "implementer decides during step 5."** Per perf-4 §6
R3 the recommendation is Fix A (cfg-gate the three sequential
tests). Pin it explicitly: "Fix A is chosen; implementer applies
`#[cfg(not(feature='threads'))]` to the three default-build tests."
This removes a load-bearing decision from execution time.

**S3. perf-5 §R8 notes `decode_action` stays AoS (`world.rs:1303`).
After perf-2 lands, `eat_and_scavenge` no longer clones the genome
(it splits hot/cold reads). But `decode_action` is called from
`pick_action_d`, which receives a `&Genome` via
`creatures.genomes[i]`. perf-5 §"Site F" keeps this AoS read. The
cross-piece note: this AoS read is one of the ~14% of bytes still
touched per tick. Acceptable. Just document that perf-5 leaves it
intentionally.

**S4. `web/src/widgets/` directory creation.** ui-perf creates it.
ui-stats §1 D4 explicitly does NOT move `stats.ts` into it
(minimum-churn). ui-inspector §4.1 does NOT move `inspector.ts`
into it either. Result: only `perf-panel.ts` lives in `widgets/`.
Acceptable but slightly asymmetric. No action needed; just be aware
the directory will contain one file initially.

---

## 4. Commit-ordering recommendation

**Keep the master plan §4 order as-is:** perf-1 → perf-2 → perf-3 →
perf-5 → perf-4 → ui-stats → ui-inspector → ui-perf.

Rationale:

- perf-1, perf-2, perf-3 are independent (any order); the
  recommended order keeps the smallest/safest first.
- perf-5 must land AFTER perf-1 (push/remove_indices merge — M1) and
  AFTER perf-2 (read-site rewrite merge — M2). The current order
  honors this.
- perf-4 must land AFTER perf-5 because perf-5 rewrites Site G
  inside the threaded NN block — if perf-4 landed first, perf-4
  would bootstrap a threaded golden that perf-5 then invalidates
  (the bit pattern is the same per perf-5's determinism claim, but
  having the threaded golden bootstrapped against the post-perf-5
  state is cleaner).
- ui-stats MUST land first among the UI pieces because it owns the
  canonical `.overlay-widget` rule AND the `#right-rail { display:
  none }` rule. ui-inspector and ui-perf both consume them.
- ui-inspector and ui-perf order is interchangeable.

**No commit-ordering changes required.**

---

## 5. Integration timeline (file-by-file)

For each commit, the files touched. Bold = files first introduced in
that commit. Strike = files where prior-commit edits get extended.

| commit | files | shape |
|---|---|---|
| perf-1 | `src/constants.rs` (move 2 consts), `src/vision.rs` (re-export + rewrite `fill_one`), `src/creature.rs` (add `eye_trig` field, `push_*`/`remove_*`/`with_capacity`/`recompute_eye_trig_at`/`swap_remove_chunk`), `src/world.rs` (none) | ~80 LOC, golden-safe |
| perf-2 | `src/world.rs` (struct + 2 inits + 2 function bodies) | ~120 LOC, golden-safe |
| perf-3 | `src/grid.rs` (1 field + `new` + `rebuild` body + test) | ~30 LOC, golden-safe |
| perf-5 | `src/creature.rs` (extends perf-1 edits: 7 mirror fields + `push_hot_mirrors` + `remove_indices` extension), `src/world.rs` (extends perf-2 edits: 6 site rewrites + threaded Site G), `src/vision.rs` (extends perf-1 edits: 4 read-site rewrites in `fill_one`) | ~250 LOC, golden-safe |
| perf-4 | **`src/lib.rs`** (1 re-export), `src/vision.rs` (extends perf-1 + perf-5: parallel `run`), **`tests/acceptance.rs`** (gate + new test), **`tests/golden_snapshot_t10000_threaded.txt`** (new pinned), **`web/src/main.ts`** (1 import + 1 await), **`.github/workflows/ci.yml`** (2 wasm-pack + 1 acceptance), **`README.md`** (1 invocation), **`DECISIONS.md`** (new section) | ~120 LOC, dual-golden |
| ui-stats | **`web/index.html`** (move 2 canvases, add `#stats-box`), **`web/src/styles.css`** (add `.overlay-widget` + `#stats-box` + `#right-rail { display: none }`) | HTML+CSS only |
| ui-inspector | `web/index.html` (extends ui-stats: add `#inspector-box`, empty `#rail-inspector`), `web/src/styles.css` (extends ui-stats: add `#inspector-box` rules; delete legacy `#rail-inspector dl`), `web/src/rail/inspector.ts` (5 edits: handle helpers, openInspector, clearSelection, refreshInspector, installCanvasClickHandler) | HTML+CSS+TS |
| ui-perf | `web/index.html` (extends prior: remove `#profiler-panel` from rail, add `#perf-box`), `web/src/styles.css` (extends prior: drop old profiler rules, add new widgetized rules), **`web/src/widgets/perf-panel.ts`** (new file with moved code), `web/src/rail/stats.ts` (delete lines 101–270 + line 6 import), `web/src/main.ts` (extends perf-4: 1 import path change) | HTML+CSS+TS, new dir |

`web/src/main.ts` is the only file touched by both perf-4 (initThreadPool import + await) and ui-perf (import path change for `installProfilerPanel`). The two edits are at different lines and disjoint. Order-independent.

`web/index.html`, `web/src/styles.css`, `web/src/main.ts` are touched by all three UI pieces. Each piece's edits are at disjoint regions per the per-plan diffs. The recommended UI order (stats → inspector → perf) means each piece rebases on the prior's edits, but no edit overwrites another.

---

## 6. Risk summary (executive view)

Highest residual risks after this review:

1. **perf-5 implementer not rebasing against perf-1 / perf-2** — M1, M2.
   Plans are written as if against `main`; merging is the
   implementer's job. Adding explicit merge notes makes this safe.
2. **`pointer-events` policy inconsistency on `.overlay-widget`** —
   M4, M5. ui-stats wants the canvas-pan-through behavior; ui-inspector
   needs click-capture. The canonical base class definition must
   pick one and the other widgets override.
3. **perf-4 R3 Fix A choice deferred to implementation time** —
   S2. Pin Fix A in the plan; remove the choice from execution.

Everything else is conventionally clean — disjoint edits, single
owners, stable contracts pinned at the source-of-truth level.

---

*End of cross-review.*
