---
status: shipped
owner: Codex orchestrator
last_updated: 2026-06-08
okay_to_delete: true
long_lived: false
owning_docs:
  - architecture/simulation-core.md
  - architecture/render-pipeline.md
  - architecture/shared-memory-and-protocol.md
  - architecture/worker-runtime.md
---

# Open-bugs fix orchestration

## Mission

Triage and fix every item currently listed in `open-bugs.md`, add focused
regression coverage, run the consolidated repository gates once, migrate durable
facts into the owning docs, and remove resolved entries from the long-lived bug
backlog.

## Scope

In scope are the seven bugs listed in `open-bugs.md` on 2026-06-07:
toroidal viewport wrapping, render ghost-copy UVs, dead `biome_buf`, dead
`tile_dirty` writes, scatter self-tile double activation, the inert
`grass_bites_per_block` slider, and restart-inert `grass_multisight`.

Out of scope are unrelated grass algorithms and the pending implementation work
in `v2.0.8-grid-rebuild-perf.md`. This run may share its final Rust gates because
the worktree is common, but must not change its pyramid cadence or grid-rebuild
decisions.

## Approach

Sequence implementation by overlapping write sets:

1. Recon all streams in parallel and confirm/refute each backlog item.
2. Run the WebGL render stream and wasm boot/config stream in parallel.
3. Run one combined Rust grass stream owning `grass/mod.rs`.
4. Red-team the toroidal fixes and cross-language boot/API removal.
5. Run one consolidated gate suite, migrate docs, and update both plans.

| Stream | Area | Status | Last observed fact | Next action | Blockers |
|---|---|---|---|---|---|
| G | Rust grass fixes | shipped | Wrapped windows, dirty deletion, scatter self-skip, and graze slider patch landed; focused and full Rust gates passed | None | None |
| R | WebGL ghost-copy UV | shipped | Signed-origin/per-ghost UV offset patch landed in `gl.ts`; web gates and e2e passed | None | None |
| W | wasm boot/config cleanup | shipped | Dead `biome_buf` API/boot fields removed; `grass_multisight` rides boot before NN layout construction | None | None |
| V | correctness review | complete | Red-team caught normalized-origin UV bug and stale bench constructors; both fixed | None | None |
| Q | consolidated verification | complete | Full Rust, clippy, bench, threaded wasm rebuild, web build, and Playwright gates passed | None | None |
| D | docs/backlog migration | complete | Owning docs updated and `open-bugs.md` cleared | None | None |

## Decisions Log

- 2026-06-07: Use a disposable execution hub rather than expanding the
  long-lived `open-bugs.md` triage list.
- 2026-06-07: Keep all `grass/mod.rs` fixes in one implementation stream to
  avoid concurrent edits and permit a single coherent regression-test pass.
- 2026-06-07: Treat v2.0.8's pending Rust gates as stale after this run's Rust
  edits; use this run's consolidated results when updating its status.
- 2026-06-08: Toroidal snapshot origins are signed logical metadata values.
  Copy-out normalizes them modulo the level dimensions; render ghost draws add
  their world translation divided by the window's world span to the base UV
  offset.
- 2026-06-07: Delete the dead `biome_buf` API and boot fields rather than retain
  a compatibility shim. The repository app is the supported consumer and has
  already stopped reading them.
- 2026-06-08: `tile_dirty` should be fully deleted, not retained under
  `cfg(test)`. Current production refresh uses the pyramid/window path and the
  dirty quantizer was test-only.

## Open Questions

None.

## Exit Gate

- Focused regression tests cover every confirmed behavioral bug.
- `cargo test --lib` and `cargo test --lib --features threads`.
- `cargo fmt --all --check`.
- `cargo clippy --all-targets -- -D warnings` and
  `cargo clippy --all-targets --features threads -- -D warnings`.
- `cargo bench --no-run`.
- `cd app/web && pnpm typecheck && pnpm build && pnpm test:e2e`.
- Threaded wasm/browser smoke if the integrated changes require live visual
  confirmation.
- Resolved entries are removed from `open-bugs.md`; durable current-state facts
  are migrated to owning docs.

## Migration Notes

- `architecture/simulation-core.md`: removed dirty-tile snapshot claims and
  documented live `grass_bites_per_block`.
- `architecture/shared-memory-and-protocol.md`: removed full-field biome buffer
  fields, added signed window origins, boot-time `grass_multisight`, and the
  wrapped clipmap contract.
- `architecture/render-pipeline.md` and `decisions/render.md`: documented
  per-ghost UV offsets for toroidal clipmap draws.
- `architecture/biome.md`: replaced boot `biome_buf` wording with the static
  `BiomePyramid` + per-slot biome window model.
- `architecture/app-shell.md`, `architecture/testing.md`,
  `decisions/sim.md`, `decisions/cross-cutting.md`, and
  `repository-layout.md`: updated affected invariants and anchors.
- `open-bugs.md`: cleared all resolved entries.

## See also

- `docs/plans/open-bugs.md`
- `docs/plans/v2.0.8-grid-rebuild-perf.md`
- `docs/plans/index.md`
