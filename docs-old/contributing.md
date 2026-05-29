# Contributing

## The spec is binding

[archive/PITCH-v5.md](archive/PITCH-v5.md) is the primary spec. [archive/PITCH-v6.md](archive/PITCH-v6.md) patches it. On any conflict, v6 wins. Both are binding for v1. Do not implement behavior that contradicts either document without a spec patch discussion.

## v1 scope is locked

v1 includes everything in v5 §14's "Included" list. Anything outside that list is v1.1 or later. If you want to add something that is not in scope, open a v1.1 issue rather than sneaking it into a v1 PR. The list of suggested v1.1 priorities is in [../BUILD-REPORT.md](../BUILD-REPORT.md).

## Decision log

Every implementation choice that is not directly specified by v5+v6 must be added to [../DECISIONS.md](../DECISIONS.md): one line stating what was decided and why. This exists so future contributors (and future Claude conversations) can understand non-obvious choices without re-reading the full build history.

## Code conventions

- **Magic numbers**: all go in [../src/constants.rs](../src/constants.rs) with a comment citing the v5/v6 section (e.g. `// v5 §7`). Nothing hardcoded inline.
- **One module per concern**: do not merge unrelated responsibilities into one file. The existing module boundaries are intentional.
- **Tests inline**: unit tests live in `#[cfg(test)]` blocks at the bottom of the relevant `.rs` file. Integration tests go in `tests/`. No separate test file for something that belongs in the module.
- **Clippy + fmt**: both must be clean. Run `cargo clippy --all-targets -- -D warnings` and `cargo fmt` before committing.
- **TypeScript**: `pnpm typecheck` must pass.

## How v1 was built (subagent pattern)

[archive/ORCHESTRATOR.md](archive/ORCHESTRATOR.md) describes the build process: a planner agent wrote a milestone plan, a reviewer approved it, an implementer built it, a reviewer checked the output, repeat for each milestone (B through F). The per-milestone plan docs are in [archive/plans/](archive/plans/). This is background context; you do not need to follow this pattern for individual contributions.

## Where to look first

**Fixing a bug:**
1. Find the relevant module (see [architecture.md](architecture.md) for the module map).
2. Write a failing test that reproduces the bug.
3. Fix the code until the test passes.
4. Make sure the acceptance test still passes (`cargo test --release --test acceptance`).
5. Log a DECISIONS.md entry if you had to make a non-obvious choice.

**Adding a feature:**
1. Confirm the feature is in v1 scope (v5 §14 "Included"). If not, it is v1.1+.
2. Find the right module(s) — [architecture.md](architecture.md) is the map.
3. Add constants to [../src/constants.rs](../src/constants.rs) with spec citations.
4. Write tests before or alongside implementation.
5. If the feature changes sim output, re-bootstrap the golden (`EVOSIM_WRITE_GOLDEN=1 cargo test --release --test acceptance`).

**Tuning balance:**
1. Sliders (live, no rebuild needed): `DevSliders` in `world.rs`, defaults in `constants.rs`, callable from JS console via `world.set_slider(name, value)`.
2. Slider ranges exposed to UI: `web/src/main.ts`.
3. After tuning, if the change is intentional, update the relevant constant in `constants.rs` and re-bootstrap the golden if sim output changed.

## Known issues

See the "Known issues" section in [../BUILD-REPORT.md](../BUILD-REPORT.md) for the full list. The most significant are:

- **F.30 NN founder init deviation** (Known Issue #1): the founder brain has hardwired energy-sensor weights (`NN_FOUNDER_*` constants) because the v6 §E uniform-random init collapses to zero ReLU output, causing immediate extinction. This is the most substantive v1 spec deviation.
- **No dev-panel UI for sliders** (Known Issue #4): use JS console (`world.set_slider(...)`).
- **Per-tick allocations** (Known Issue #5): soft-repulsion and eat/scavenge Vecs allocate fresh each tick; acceptable at current populations but is the obvious profiling target for v1.1.
