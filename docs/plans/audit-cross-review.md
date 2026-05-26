# Audit cross-review (2026-05-24)

Reviewer: opus (cross-piece-conflict reviewer).
Scope: 13 per-piece plans + audit-master + audit-triage. Checks for inter-piece
conflicts, integration risks, and dependency-graph violations a single-piece
reviewer would miss.

## Overall verdict

**APPROVE WITH REVISIONS.** No structural blockers; the 4-PR + plan-only-WebGL2
shape is coherent and the dependency graph is well-modelled. Two NEEDS-FIX items
(the webgl2-renderer-design.md B1 stride mismatch already self-flagged by its
own reviewer; the S37 twox-hash bootstrap timing that is master-plan-noted but
not echoed in S7/S8 plans) require text-only plan updates before
implementation. Three TBD items defer to bootstrap results (S37, S24, the
sequential==threaded golden equality post-PR-3 regen). The remaining
checklist items pass.

---

## Findings (per checklist item)

### (a) World.rs split path-translation — **PASS WITH ONE NIT**

The S1 plan's §6 path-translation table is comprehensive and explicitly
called out as "the load-bearing artifact for every other PR-1/PR-3/PR-4
piece." Spot-checks across downstream plans:

- **S7** (`audit-s7-snapshot-hash-coverage.md` §8) — correctly notes "S1
  does NOT split src/snapshot_hash.rs"; uses `crate::world::World` import
  which re-exports cleanly from `src/world/mod.rs`. PASS.
- **S8** (`audit-s8-rng-hash-direct.md` §7) — correctly notes
  `src/snapshot_hash.rs` and `src/rng.rs` stay at top-level paths. PASS.
- **S11** (`audit-s11-extract-overlap-wall.md` §4) — references post-S1
  module map exactly per S1 §2 (helpers move to `src/world/nn.rs` as
  free fns); includes fallback for pre-S1 layout. PASS.
- **S12** (`audit-s12-validate-save.md` §6) — correctly notes `src/save.rs`
  is not split; `from_save_v1` moves to `src/world/save_v1.rs`. PASS.
- **S17** (`audit-s17-typed-set-slider.md` §5) — `src/wasm_api.rs` not
  split; correctly noted. The plan's review feedback item #5 already
  flags that pre-S1 line cites will drift; reviewer recommends method-
  name anchors. NIT (plan-internal, already self-flagged).
- **S18** (`audit-s18-stable-id-creature-at.md` §6) — `src/wasm_api.rs`
  not split; PASS.
- **S21** (`audit-s21-drop-unused-flag-floats.md` §5) — `src/wasm_api.rs`
  + `web/src/render.ts` not touched by S1; PASS.
- **S23** (`audit-s23-threaded-nn-par-chunks-mut.md` §6) — references
  post-S1 `src/world/nn.rs`; explicit fallback if S1 placed the function
  elsewhere. PASS.
- **S24** (`audit-s24-scavenge-cell-to-carrion.md` §7) — references
  post-S1 `src/world/tick.rs` for `eat_and_scavenge`. PASS.
- **S26** (`audit-s26-cell-to-carrion-csr.md` §5) — references post-S1
  paths and notes S11 collapses two reader sites into one. PASS.
- **S39** (`audit-s39-equivalence-and-save-load-tests.md` §6) — places
  test (c) in `src/world/nn.rs` per S1; includes pre-S1 fallback. PASS.

The S1 plan's own review feedback item #1 notes line numbers in S1's §2
and §6 are 2–7 off from the live file; the planner's caveat that
"line numbers are approximate" mitigates downstream impact, since
downstream plans use file-and-function references rather than literal
line numbers when consulting the table. NIT.

### (b) S7 + S8 single-commit landing — **PASS**

- `audit-s7-snapshot-hash-coverage.md` §7 explicitly: "This piece lands
  in the same commit as S8. … One regen ceremony at end of PR-3 is
  the master-plan rule. If S7 ships first and S8 second (two commits,
  two regens), we've spent two regen ceremonies and obscured which
  fields drove which delta."
- `audit-s8-rng-hash-direct.md` §4 explicitly: "Pair: lands in **one
  commit with S7**."

Both plans share an aligned commit-message template and defer to
`audit-master.md` §8 for the single regen ceremony. PASS.

### (c) S11 helper signatures match S23's needs — **PASS**

- S11 §2 declares the chosen signatures:
  `count_carrion_overlap(creatures: &CreatureSoA, carrion: &[Carrion],
   cell_to_carrion: &[Vec<u32>], i: usize) -> u32` and
  `compute_is_at_wall(creatures: &CreatureSoA, i: usize) -> f32`.
- S23 §4 ("Required signatures") demands exactly these signatures and
  flags as a risk (§9 R4) what to do if S11 ships methods instead.
- S23's own review feedback ("Verified") confirms the signature match.

Both plans are aligned. Both take immutable borrows only, so neither
collides with S23's `mem::take` of the three output columns. PASS.

### (d) S24 hash-drift bootstrap — **PASS**

`audit-s24-scavenge-cell-to-carrion.md` §5 ("Bootstrap-first workflow —
mandatory ordering") spells out the orchestrator-report sequence in
six numbered steps, including: "Report to the orchestrator: If hash
matches `0xb76e907c6221f7f5`: S24 lands in PR-3 standalone … If hash
differs: report the new hex value." PASS.

### (e) Dual-golden equality post-PR-3 — **PASS**

No per-piece plan asserts a literal post-regen hash value. All plans
defer to master plan §8:

- S7 §9 "The post-S7+S8 hashes go into `DECISIONS.md` per
  audit-master.md §8."
- S8 §8 "New hashes will differ from `0xb76e907c6221f7f5`."
- S11 §9 "Both golden tests on both feature builds still produce
  `0xb76e907c6221f7f5`" — but only as the **pre-PR-3 baseline** check;
  S11 itself is verify-then-decide and does not regen.
- S21 §4 confirms goldens unchanged (no determinism impact).
- S23 §8 "the threaded golden must remain at whatever value PR-3 ends at."
- S26 — no value asserted.
- S39 §3 explicitly contemplates both equal and divergent post-regen
  outcomes and proposes a DECISIONS.md note for divergence.

PASS. **Caveat:** S39's reviewer issue B1 raises a substantive concern
about test (a)'s framing (compare-to-golden tautology vs actually
running both code paths) — this is a quality issue inside S39, not a
cross-piece conflict. The DECISIONS-line discipline is intact.

### (f) S37 twox-hash 1→2 bootstrap — **NEEDS-FIX (minor)**

Master plan §5 PR-1 S37 entry instructs a bootstrap check before
PR-3 to verify byte-stability of XXH64 across the crate bump. S7 plan
review-feedback item B2 flags an endianness subtlety in twox-hash 1.6's
default `write_u64` (it uses `to_ne_bytes`, not `to_le_bytes`) and
recommends a NOTE comment near the new `write_u32`/`write_u64` calls.
**The S7 and S8 plans should explicitly cite the S37 bootstrap as a
dependency** so that if S37 has not yet been bootstrap-verified when
S7/S8 are implemented, the implementer knows to escalate.

Currently:
- `audit-s7-snapshot-hash-coverage.md` does NOT reference S37 or the
  twox-hash bump.
- `audit-s8-rng-hash-direct.md` §11 mentions "S37 in PR-1 bumps the
  crate version but not the algorithm" and §3 leans on `write_u64`
  semantics that the S7 reviewer flagged as needing verification under
  v2.

**Action:** add a one-sentence dependency note in S7 §8 and S8 §7
acknowledging that the **S37 bootstrap result must be confirmed before
PR-3 begins**, per master plan §7 (f). If S37 drifts the hash, S37
moves to the PR-3 regen batch and the regen ceremony captures
everything together. This is plan-text-only; no code change.

### (g) S21 stride math single-commit — **PASS**

`audit-s21-drop-unused-flag-floats.md` §3 explicit: "Land all steps in
one commit (the runtime assert and the stride change MUST ship
together — see Risk (a))." The TS-side runtime assert
(`EXPECTED_CREATURE_STRIDE = 11`) is mandated alongside the Rust
`creature_stride() -> 11` change. Risk (a) restates this and §8
acceptance criteria includes it. PASS.

### (h) S26 CSR vs S24 reader — **PASS**

Both plans reference each other:
- S24 §4 ("Layout coordination with S26"): "PR-4 S26 changes the field
  to `CarrionIndex { starts, indices }` CSR and is responsible for
  translating this reader site (and every other reader) to
  `&indices[starts[cell] .. starts[cell+1]]`. The S26 plan doc
  explicitly notes this touchpoint."
- S26 §4 row 4 explicitly lists S24's new site: "S24 ships in PR-3
  with `for &ci in &self.cell_to_carrion[cell_idx] { … }` inside a 3×3
  sweep … New: `for &ci in self.cell_to_carrion.cell(cell_idx) { … }`
  — preserve the y-outer, x-inner 3×3 sweep order exactly as S24 wrote
  it."
- S24 §9 risk (e) reinforces by noting S24 inlines the sweep (no
  helper), so S26's `grep self.cell_to_carrion[` will catch it.

PASS.

### (i) S18 + S20 coordination — **PASS**

`audit-s18-stable-id-creature-at.md` §5 ("Coordination with S20"):
"Recommendation: land both pieces in one commit." Notes both touch
`creature_at`'s body. Master plan §5 PR-2 S20 inlined paragraph
describes only the body change (grid-backed scan), does NOT specify
return type — so it is compatible with S18's return-type change.

S18's reviewer issue #8 confirms the one-commit recommendation is
correct and not a giant diff (~80 LOC combined).

One small wrinkle: S20 inline brief says "Care: the grid is rebuilt at
tick-step-1 from start-of-tick positions; if `creature_at` is called
mid-frame, the grid is already current for THIS tick." S18 does not
contradict this. PASS.

### (j) WebGL2 design vs S21 stride — **NEEDS-FIX (already self-flagged
as B1 blocker in the design doc's own review)**

`webgl2-renderer-design.md` §3.3 describes a post-S21 layout that
contradicts S21's actual schema:

- WebGL2 design §3.3: post-S21 record is 7 fields, with `ring_mask`
  (uint, packed bits).
- S21 §2 actual post-S21 record: 11 floats, with **five separate flag
  floats** (`flag_eye`, `flag_move`, `flag_scav`, `flag_mouth`,
  `flag_armor`) — NO packed mask.

This is documented as B1 in the WebGL2 design's own review feedback
section (Reviewer notes "blocker" severity). Because WebGL2 is
plan-only this pass (no code lands), the conflict is **not a current
implementation risk** — but the design doc MUST be reconciled before
any future implementation orchestration kicks off, OR a new
post-S21 piece "S21b: pack flag floats into u8 ring_mask" must be
added to a future pass and the WebGL2 doc updated to depend on it.

**Action:** WebGL2 design author must revise §3.3 / §3.4 / §3.5 per
the doc's own review summary. No effect on this pass's
implementation, but the cross-reviewer should record this so the next
orchestration doesn't trip on it.

### (k) S23 N_CHUNKS preservation — **PASS**

`audit-s23-threaded-nn-par-chunks-mut.md` §11 ("Locked scope"):
"Do not change chunk count away from `N_CHUNKS = 8`." §3 reinforces
that the chunk count must equal what `chunk_ranges(n)` produces. §9
R3 makes "any change in chunk count (other than N_CHUNKS = 8) breaks
the threaded golden" the hard rule. PASS.

### (l) S39 test (a) landing order — **PASS WITH MINOR PLAN-NOTE**

`audit-s39-equivalence-and-save-load-tests.md` §7 specifies "test (a)
… Depends on the PR-3 regen ceremony … having pinned BOTH … to their
new post-S7+S8 values. **The test must NOT land before regen** …
The orchestrator should schedule S39 as the last commit in PR-3
(after the regen ceremony), or split it: tests (c) and (b) land
before regen, test (a) lands after."

S39's own reviewer (last block) requests a master-plan §8 update to
add an explicit step 5 for "schedule S39 test (a) as the final commit
of PR-3 after regen-and-verify pass."

**Action:** master plan §8 should add the explicit ordering. NIT
(master-plan text update, not a per-piece plan blocker; S39 already
captures the ordering correctly in its own plan).

### (m) #[allow(dead_code)] removal in S11 surfacing other warnings — **PASS**

`audit-s11-extract-overlap-wall.md` §7 risk (c): "Removing
[`#[cfg_attr(... allow(dead_code))]`] may reveal latent dead-code
warnings." The annotation is narrow (`feature = "threads"` only on
the two helper methods). Post-S11 these helpers are reachable from
both call sites, so they themselves stop being dead.

Cross-check: no other PR-3 plan introduces a `#[cfg(not(...))]`-gated
function that would surface as dead code under the `--features
threads` clippy run. PR-4's S23 also doesn't introduce dead code under
default features. PASS — no collateral dead-code surfacing is
expected. The S11 review confirms this in non-issue (7).

### (n) S25 scratch_eat_candidates vs S26 CSR — **PASS**

S25 is inlined in master plan §5 PR-4 — it's about reusing a `Vec`
across ticks in the `Eat`-handling loop. It does NOT read from
`cell_to_carrion`; the eat loop uses `grid.for_each_in_radius` which
ranges over creatures (via `SpatialGrid`), not carrion. The CSR change
in S26 is on `cell_to_carrion` (carrion-only). No collision.

S25's preferred form ("inline-in-callback") doesn't allocate at all,
so even less interaction. PASS.

### (o) S12 validate_save + S39 test (b) — **PASS**

`audit-s12-validate-save.md` §5 includes the positive test:
`validate_save_accepts_real_world_save` runs `World::new(seed).to_save_v1()`
and asserts `validate_save(&save).is_ok()`, then also a t=500 stepped
world. §9 ("Pair-with notes") explicitly: "S39 (test piece in same
PR-3) adds `save_load_hash_equal_immediately_after_load`. That test
runs `World::new(seed).snapshot()` → save → load → re-hash, asserting
equality before any `step()`. S12's validator MUST pass for every
legitimate save the test produces — if `validate_save` rejects any
output of `to_save_v1`, S39 fails immediately. This is the integration
canary."

S39 §4 ("Interaction with S12") mirrors this: "S12 (`validate_save`)
hardens `from_save_v1` against malformed inputs but does NOT change
accept-path semantics for valid saves. Test (b) flows entirely
through the accept path."

PASS.

### (p) Single-commit rule for paired pieces — **PASS**

- **S7 + S8**: both plans require one commit (§7 of each). PASS per
  checklist item (b).
- **S18 + S20**: S18 recommends one commit (§5); S20 inline brief is
  body-only and compatible. PASS per (i).
- **S21 (Rust stride + TS assert)**: one commit per (g). PASS.

No other pair in the plan set requires single-commit landing that
wasn't called out. S11 stands alone (its two call-site updates are in
one commit by construction since both live in `src/world/nn.rs`); S24
+ S26 are split deliberately (S24 in PR-3, S26 in PR-4 — already
documented as a coordinated multi-PR pattern in (h)). PASS.

---

## Additional findings (anything not on the checklist)

### A1. S37 twox-hash bump is the most fragile cross-cutting dep

Master plan PR-1 includes S37 inline. The S37 entry already includes
the bootstrap-check instruction. But the orchestrator handoff between
PR-1 and PR-3 needs to surface the bootstrap result explicitly. The
S7 reviewer's B2 finding (twox-hash 1.6's `write_u64` is
`to_ne_bytes`, not `to_le_bytes`) is non-blocking on x86_64+wasm32 (both
LE), but if twox-hash 2.x changes the default trait impl, the hash
bytes could shift independent of any per-piece change. **Action:** the
orchestrator should treat the S37 bootstrap as a precondition gate
before kicking off any of the PR-3 planners (S7/S8 in particular), and
record the bootstrap hash in `DECISIONS.md` even if it matches the
pre-bump value.

### A2. S39 test (a) framing critique (already in S39 review)

S39 reviewer B1: test (a) reads two golden files and asserts their
equality, but both files are regenerated in the same §8 ceremony, so
the test is largely a regen-ceremony sanity check rather than an
actual sequential-vs-threaded runtime comparison. The S39 plan author
should adopt the reviewer's recommendation (i): document the test as
a divergence canary, not as a runtime cross-mode comparator. This is a
plan-internal issue; not a cross-piece conflict, but it affects the
master-plan-level claim that "sequential==threaded is a hard
invariant."

### A3. S39 test (b) cfg-gate hides threaded-load drift (S39 review S1)

S39 reviewer S1 recommends making test (b) cfg-agnostic so it runs in
both feature builds. The current `#[cfg(not(feature = "threads"))]`
gate inherits from an unrelated F.26 test pattern. This is also
plan-internal but the master plan §10 acceptance gates expect both
feature builds to pass; running save/load only under one is a
coverage gap that the orchestrator should request the S39 planner to
close.

### A4. S26 §4 row 4 silently assumes S24's break label

`audit-s26-cell-to-carrion-csr.md` §4 row 4 says "preserve the
y-outer, x-inner 3×3 sweep order exactly as S24 wrote it." The S24
plan uses `'sweep:` as the break label name (§6.2 snippet, §9 risk
(f)). S26's grep for `self.cell_to_carrion[` will catch the reader
line but the surrounding `'sweep:` label is unchanged by S26 — no
issue, just noting that the S26 implementer must NOT remove the label
when translating the reader idiom. The plans are aligned; this is a
courtesy note for the implementer.

### A5. S12 validate_save scope expansion (S12 review N1, N2)

S12's own reviewer requests two non-blocking rule additions:
`brain.nn_mutation_rate` finite check (N1) and `DevSliders` f32
finite check (N2). Neither changes any cross-piece interface. The
master plan §4 S12 brief specifies 6–7 rules; the reviewer expands
to ~8. Either is fine, but the S39 test (b) integration canary
ensures the validator stays accept-path-symmetric regardless of which
set of rules ships. No cross-piece blocker.

### A6. The "scratch field uplift" cascade from S1

S1 §3.1 uplifts nine scratch fields and two formerly-private fields
(`cell_to_carrion`, `pending_extinction_check`) from private to
`pub(crate)`. S35 (PR-1 inlined) tightens `pub` → `pub(crate)` across
modules. Order matters: S1 must land first (creating the new
pub(crate) surface), then S35 can tighten the rest. Master plan §6
PR-1 dep graph captures this ("S35 (pub(crate) tightening — needs
post-split surface)"). PASS, but if S35's implementer accidentally
re-tightens an S1-uplifted scratch field to private, the tick code
won't compile under `--features threads`. The S35 inline brief should
explicitly carve out the S1-uplifted fields. NIT.

### A7. WebGL2 doc B2 and B3 (already self-flagged)

Beyond the stride mismatch (B1, checklist (j)), the WebGL2 review
also flags B2 (ring-pack offset scheme using the OLD `ringR -= 1.5`
math doesn't translate cleanly to instanced rendering with N_RINGS
fragments per creature) and B3 (body draw path missing). These are
plan-internal and do not impact this audit pass; future
implementation orchestration must resolve them. Noted for the
master-plan record.

### A8. S26 introduces a new `cursors: Vec<u32>` field on `CarrionIndex`

S26 adds a `cursors` scratch field to `CarrionIndex` (mirroring the
perf-3 trick for `SpatialGrid`). This is a new allocation site at
construction time (vec![0u32; HASH_DIM*HASH_DIM + 1]). It is reused
across rebuilds (no per-call alloc). No conflict with S25's
scratch-pool pattern; both follow the same "promote to field, reuse
via clear+resize or copy_from_slice" recipe. PASS, but the S26 plan's
acceptance criteria don't explicitly count this allocation; a careful
allocation-budget reviewer might want it noted in DECISIONS.

---

## Blocking issues requiring plan updates

Strictly speaking, **zero hard blockers** for this pass (WebGL2 design
issues are not blockers because no WebGL2 code lands). Plan-text
updates desired before implementation begins:

1. **S7 plan §8** (and S8 plan §7): add an explicit dependency note on
   the S37 bootstrap result. One sentence each. Per checklist item (f).
2. **Master plan §8** (regen ceremony): add a step 5 noting that S39
   test (a) lands as the **final** commit of PR-3, after regen-and-
   verify. Per checklist item (l) and S39's own reviewer note.
3. **WebGL2 design §3.3 / §3.4 / §3.5**: reconcile post-S21 layout
   (11 floats with five separate flag floats) with the design. NOT a
   blocker for this pass (no code lands); IS a blocker for the
   next-orchestration WebGL2 implementation. Per checklist item (j).
4. **S39 plan §3 (test a) and §2 (test b cfg gate)**: address
   reviewer's B1 (test (a) framing) and S1 (test (b) cfg gate) per
   additional findings A2 and A3. Not cross-piece blockers, but the
   master-plan claim "sequential==threaded as a hard invariant"
   weakens until B1 is addressed.

Optional (NIT) plan-text updates:

- S1 §2 / §6 line-number drift (S1 reviewer issue 1): refresh
  pre-split line numbers with `grep -nE '^(impl|fn|pub fn|...)' src/world.rs`.
- S17 plan: switch line cites to function-name anchors (S17 reviewer
  nit 5).
- S35 inline brief: carve out the S1-uplifted scratch fields so a
  future re-tightening doesn't break the tick path.

---

## Recommended implementation sequence adjustments

Master plan's PR ordering (1 → 2 → 3 → 4, with WebGL2 design in
parallel with PR-4) is sound. Two refinements:

1. **Within PR-1, run S37 bootstrap FIRST (before any other PR-1 code
   lands).** If S37 flips the hash, all of PR-3's hash-touching pieces
   need to re-plan around it. Currently master plan §6 PR-1 dep graph
   shows S37 as parallel-with-S1; promote it to a sequential
   pre-S1 step so the orchestrator gets the bootstrap result before
   PR-3 planners are kicked off. (Per checklist (f), additional
   finding A1.)

2. **Within PR-3, schedule the commit order as:**
   a. S1 already landed in PR-1.
   b. PR-3 byte-identity pieces in any parallel order: S4, S5, S6, S9,
      S10, S11, S12, S39 tests (b) and (c).
   c. S24 bootstrap-and-decide (report to orchestrator).
   d. S7 + S8 in one commit (the regen-driving pair).
   e. Run the §8 regen ceremony exactly once.
   f. S39 test (a) lands as the final commit, asserting against the
      newly-pinned goldens.
   g. PR-3 cross-review.

   This ordering matches what the per-piece plans already imply but
   makes the orchestrator's job mechanical.

3. **PR-2 single-commit pair (S18 + S20).** Land per S18 §5
   recommendation. The cross-reviewer at PR-2 end confirms the
   post-merge `creature_at` body uses grid (S20) AND returns
   `Option<f64>` (S18).

4. **PR-2 single-commit pair (S21 Rust + TS).** Per checklist (g) and
   S21 §3 — already specified, just emphasize at orchestrator handoff.

5. **PR-4 ordering:** S23 depends on S11 (already landed in PR-3).
   S26 depends on S24 (already landed in PR-3) and S11. S28 depends on
   S27. The master plan §6 PR-4 dep graph captures this; no change.

---

## Cross-reviewer's confidence

High. The 13 per-piece plans are individually thorough and the
cross-piece coordination notes (especially the recurring `audit-master
§7 watchlist` callouts in S7, S8, S11, S21, S23, S24, S26) demonstrate
that the planners read each other's plans during drafting. The
verify-then-decide stance on S11 and S24, the bootstrap-first
discipline on S37, and the single regen ceremony at end of PR-3 are
all the right disciplines for a pass this large. The two genuine
NEEDS-FIX items (S37 bootstrap dependency note in S7/S8; WebGL2
post-S21 layout) are plan-text edits, not structural rewrites.

The single most important orchestrator action is to **run the S37
twox-hash 1→2 bootstrap before kicking off PR-3 planning** so that the
implementer team knows up front whether the hash bytes drift from the
crate bump or only from S7+S8.
