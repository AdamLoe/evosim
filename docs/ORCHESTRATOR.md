# Orchestrator handoff — evosim v1 build

You are the orchestrator for the v1 build of evosim. This document is the durable brief; the user may paste a short opener that points here.

## Mission

Ship a working v1 of evosim per [PITCH-v5.md](PITCH-v5.md) and [PITCH-v6.md](PITCH-v6.md). v1 is a browser-deployed idle evolution sandbox, Rust→wasm with a TS shell, runnable from a single URL. The definition of done is **v5 §16** — a headless acceptance test plus a small set of manual UI checks.

You should ship v1 **in one orchestration pass**. Do not stop mid-build to ask the user questions; aggregate any questions for end-of-build delivery. The user has explicitly authorized burning Claude compute on this build.

## Read first

Before doing anything else, read both pitch documents in full:

1. **[PITCH-v5.md](PITCH-v5.md)** — design source of truth. Mechanics, brain, energy economy, milestones, definition of done.
2. **[PITCH-v6.md](PITCH-v6.md)** — patches + binding implementation defaults. Supersedes v5 on any conflict. All the small picks (frontend framework, σ_t scales, RNG, eye-count adjacency, soft-repulsion formula, etc.) are pinned here.

There are also v1–v4 PITCH files in [docs/](.) showing how the design evolved. They are historical only. v5+v6 are authoritative; ignore the earlier versions unless you need context on *why* a decision was made.

The [docs/original_idea_docs/](original_idea_docs/) folder contains the user's pre-planning notes and is also historical only.

## Repo state at handoff

- `/home/adamg/evosim/` is the project root.
- It is **not currently a git repo**. Your first action in Milestone A should be `git init` and an initial commit of the existing docs.
- There is no code yet. Only `docs/`.
- The user's machine is Linux (WSL2), Node and Rust toolchains are assumed available; if a tool is missing, install it via the standard channel and document in `DECISIONS.md`.

## Operating rules

1. **v5 §14 "Included" list is the scope ceiling.** You may not add features, sliders, traits, actions, or UI affordances beyond what v5/v6 explicitly list. v5 §15's "out-of-scope guardrails" reinforces this — read it carefully.

2. **No mid-build questions to the user.** If a decision is not covered by v5+v6, *you make the call*, and record it in `DECISIONS.md` at the repo root with a one-line rationale. Aggregate any genuinely blocking issues for end-of-build delivery.

3. **Milestone order is binding.** v5 §15 milestones A–F run sequentially. Within a milestone, only parallelize where v5/v6 *explicitly* labels parallel-safe (E.21–E.24 UI panels, F.26 persistence after D stabilizes). Milestone C is sequential — do not parallelize it.

4. **Stop performance work when §16 passes.** The headless acceptance test (10k ticks at 1500 creatures in <8 wall-seconds, golden snapshot hash match) is the perf gate. Do not nerd-snipe on SIMD/rayon micro-optimizations past that.

5. **Record uncovered decisions in `DECISIONS.md`.** Format: one line per decision, `<topic>: <choice> — <one-line why>`. Example: `cargo workspace layout: single crate not workspace — simpler for v1 size`.

6. **Don't skip tests.** Each module gets at least a smoke test. The headless acceptance test in M F.29 must actually run in CI.

7. **Don't skip git commits.** Commit after each milestone (and within milestone at reasonable checkpoints). Clean, descriptive messages. Don't force-push, don't amend.

## Subagent orchestration pattern

The user wants you to keep your own context at the high level (milestone plans, integration points, status across modules) and delegate depth to subagents. Use this pattern per milestone:

### Per-milestone flow

For each milestone (or substantial step within a milestone):

1. **Planner subagent** — produces a detailed plan: file layout, function signatures, integration points with prior milestones, test plan. Output: a plan blurb to you. Brief the planner with: the milestone goal, the specific v5/v6 sections that govern it, what's already been built, what comes next.

2. **Plan-review subagent** (skip for trivial milestones) — critiques the plan for spec compliance, missing edge cases, integration issues. Output: approve or revise notes. Brief the reviewer with the plan + the same spec sections.

3. **Implementer subagent** — implements per the approved plan. Output: file list, test results, any `DECISIONS.md` entries, brief summary. Brief with the approved plan, the relevant spec sections, and the project's existing conventions (Cargo layout, module style, etc.).

4. **Code-review subagent** (skip for trivial milestones) — reviews for spec compliance, bugs, missing tests. Output: approve or fix list. If fixes needed, hand back to implementer subagent.

### Adapting review depth per milestone

Per the user's preference (PITCH v4 decision: "Adaptive: orchestrator decides per module"), pick review depth by complexity:

- **Light (skip plan-review and code-review):** Milestone A scaffolding, F.27 seed display, simple UI panels in E.
- **Medium (one plan-review pass, one code-review pass):** most milestones. Default.
- **Heavy (multiple review rounds):** Milestone B sim core (energy conservation correctness is critical), Milestone D NN inference (SIMD + rayon determinism), Milestone F.26 persistence (double-buffered transferable handoff is subtle), Milestone F.29 acceptance test (the perf gate).

### Subagent briefing principles

Each subagent starts with zero conversation history. Brief them like a smart colleague who just walked in: cite specific section numbers in v5/v6, state what's already built, state what they're producing, state what comes next. Don't make them re-derive the design.

### Subagent model selection

The Agent tool's `model` parameter is set per-subagent. Use it:

- **Planner subagents → `opus`.** They need strong judgment on architecture, integration points, edge cases. Fewer of them; quality matters.
- **Plan-review subagents → `opus`.** Sharp critical reading of plans against spec. Quality matters.
- **Implementer subagents → `sonnet`.** Producing focused code from a clear plan. Faster, cheaper, code-strong. Many of them; speed compounds.
- **Code-review subagents → `opus`.** Catching subtle spec-drift, bugs, missing tests. Quality matters here too — a sloppy review lets bad code through.
- **Trivial mechanical tasks** (rename files, run a build, format) → `sonnet` or `haiku`.

Rule of thumb: **opus for judgment, sonnet for execution.** The orchestrator itself runs at opus. If a subagent's output looks weak, retry it at opus regardless of role.

### Parallelism

Spawn parallel subagents only where v5/v6 says parallel-safe (E.21–E.24, F.26 once D stabilizes). For everything else, sequential — even if "logically" parallelizable, the integration cost of merging concurrent work is usually higher than the wallclock savings on a project this size.

## Milestone-level guidance

### Milestone A — walking skeleton
- `git init`, first commit of `docs/`.
- Cargo project, single-crate or workspace (your call, document in DECISIONS).
- Pinned deps from v6 §A.
- `wasm-pack build` works.
- Vite + pnpm scaffold serving a static page.
- A `<canvas>` rendering one bouncing circle driven from Rust through wasm.
- COOP/COEP headers config for Cloudflare Pages (`_headers` file).
- CI: GitHub Actions or similar, runs `cargo check`, `wasm-pack build --target web`, `pnpm build`, lints.

### Milestone B — sim core (heavy review)
Energy conservation is the load-bearing correctness property. The photosynth two-pass in v5 §3.5 step 5 must be implemented exactly as written (demand → proportional payout → drain). Hardcode the "split when energy > threshold" placeholder behavior; no NN yet.

### Milestone C — body genome + mutation (sequential within)
C.10 → C.11 → C.12 → C.13 → C.14, in order. Do not parallelize.

### Milestone D — brain (heavy review)
- CPU SIMD with `wide` for the dense matmuls.
- Rayon parallel with the deterministic chunking from v6 §J.
- NN mutation uses geometric-skip sampling per v6 §E.
- Wire all 136 inputs per v6 §E.
- Validate behavior on a smoke test (1k creatures, sim doesn't explode, energy stays bounded).

### Milestone E — observability
- E.20 (species detection) is the sequential prerequisite. Implement the full §12 distance metric with v6 §H scales.
- E.21–E.24 (three overlay panels + in-aquarium toasts/rings) are parallel-safe.

### Milestone F — persistence & polish
- F.26 (persistence) needs double-buffered SoA + transferable ArrayBuffer handoff to a worker per v5 §13 / v6 §I. This is subtle — heavy review.
- F.29 (acceptance test harness) builds the headless test. Must hit perf budget.
- F.30 (balance tuning) **cannot be subagented** — you, the orchestrator, watch the sim and decide if defaults need adjustment. Tune only constants explicitly listed as sliders in v6 §K; do not retune anything outside that set.

## End-of-build deliverables

When v1 is shipped (or you've run out of runway), produce a final report containing:

1. **Status of v5 §16 DoD** — each criterion: pass / fail / skipped (with reason).
2. **`DECISIONS.md` summary** — the running list of choices you made for things v5+v6 didn't cover.
3. **Known issues list** — bugs, rough edges, perf gotchas, anything the user should know.
4. **Questions for the user** — anything you wanted to ask mid-build but held until the end.
5. **Suggested v1.1 priorities** — based on what you learned, what should the next pass tackle first.

## Failure handling

- If a milestone seems genuinely impossible as spec'd, log it in `DECISIONS.md`, implement the best alternative you can, and surface it in the end-of-build report.
- If you find a bug in v5/v6 itself (logical contradiction, impossible constraint), do the same — pick the most reasonable interpretation, document the call, surface in the report.
- If a test fails and you can't fix it after two attempts, leave it failing with a `#[ignore]` or skip marker, document the failure, and proceed. The acceptance test in §16 is the only test where "stop and fix" is the right call.
- If something would require breaking scope (e.g. you genuinely cannot ship the brain without adding an input), surface it in end-of-build, do not just expand scope.

## On the user's side

The user (Adam) is hands-off during the build. They've explicitly chosen to invest Claude compute here and want a working v1 they can iterate on. They prefer terseness over verbosity. They value being told the truth about tradeoffs over reassurance. They will absolutely notice if you add features beyond scope; do not.

When you produce the end-of-build report, write it like you'd write to a senior engineer who already knows the project — short, direct, no padding.

## Start

Once you've read v5 and v6, begin Milestone A.
