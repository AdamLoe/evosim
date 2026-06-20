# Implement UX/perf plans hub

Created: 2026-06-17

## Request

Implement these tracked plans:

- `docs/plans/creature-survey-lod-controls.md`
- `docs/plans/grass-no-grass-zone-toggle.md`
- `docs/plans/repeat-camera-rendering.md`
- `docs/plans/tps-stutter-diagnosis.md`

## Lifecycle

Tracked plan lifecycle. The user supplied existing plan files, and the work
crosses rendering, app shell controls, simulation/grass behavior, performance
diagnosis, e2e coverage, and architecture/decision docs.

Plan authoring and plan review are skipped for this pass because the task is
to implement named plans already present in the tree. Implementation and work
review are required.

## Initial state

`git status --short` showed a large dirty tree before implementation began,
including the four requested plan files as untracked. Treat existing source,
docs, and test edits as user/in-progress state. Do not reset or revert them.

## Streams

| Stream | Area | Status | Last observed fact | Next action | Blockers |
|---|---|---|---|---|---|
| Implement named plans | Render/app shell/sim/perf/docs | complete | Four named plans implemented, doc-migrated, and marked shipped | Final gate/commit | none |
| Review shipped work | Final verification/docs/commit review | complete | Review found implementation closeable after two doc wording fixes | Report commit safety blocker | same-file dirty overlap prevents a clean filename-stage commit |

## Decisions

- Use one implementation worker for all four plans to avoid same-file write
  conflicts across `app/web/src/main.ts`, renderer files, settings/devpanel,
  shared tests, and docs.
- Preserve pre-existing dirty worktree state and integrate with it.

## Evidence Log

- 2026-06-17: Intake loaded manifest, index, overview, orchestration rules,
  and plan lifecycle.
- 2026-06-17: Dispatched implementation worker `Avicenna`
  (`019ed3ba-b81c-72c1-95c3-730e25ca5d67`) for all four named plans.
- 2026-06-17: Implemented no-grass-zone construction toggle, repeat-camera
  coverage-derived tiles, survey-dot/border Display controls, and TPS diagnosis
  evidence. Regenerated Rust→TS world-config bindings.
- 2026-06-17: Focused verification passed:
  `cargo test --lib world::grass_biome_tests::disabling_no_grass_zones_uses_plain_capacity_everywhere`,
  `cargo test --bin gen-bindings bindings_in_sync`, `pnpm typecheck`,
  targeted Playwright `defaults/render-camera/target TPS` checks, and a manual
  dev-server settings persistence smoke on `127.0.0.1:47822`.
- 2026-06-17: Fixed-port `settings-persistence` Playwright rerun on `:47821`
  failed because the reused server was a stale production build without the
  dev-only `__evosimSettingsForTests` hook; the equivalent smoke passed on a
  fresh Vite dev server on `:47822`.
- 2026-06-17: Remaining gates passed: `cargo fmt --all --check`,
  `cargo clippy --all-targets -- -D warnings`,
  `cargo clippy --all-targets --features threads -- -D warnings`,
  `cargo test --lib`, `cargo test --lib --features threads`,
  `cargo bench --no-run`, `pnpm build`, and `pnpm docs:lint`.
- 2026-06-17: Local commit deferred. Required files such as
  `wasm_api/mod.rs`, `world/mod.rs`, `settings.ts`, `devpanel.ts`, generated
  bindings, and render/docs files contain plan changes interleaved with
  pre-existing unrelated dirty edits. A filename-stage commit would either pull
  in user/in-progress work or omit companion dirty files needed by the tested
  tree.
- 2026-06-17: Dispatched review worker `Noether`
  (`019ed3d7-7d36-7b42-a316-1f6e0cb81d6f`) to verify implementation,
  inspect plan/doc migration, and reassess commit safety.
- 2026-06-17: Closure review verified the no-grass-zone construction path,
  repeat-camera tile math, and TypeScript typecheck. Focused checks passed:
  `cargo test --lib world::grass_biome_tests::disabling_no_grass_zones_uses_plain_capacity_everywhere`,
  `pnpm exec playwright test tests/e2e/render-camera.spec.ts --reporter=line`,
  `pnpm exec tsc --noEmit`, and `pnpm docs:lint`.
- 2026-06-17: Review fixed stale architecture wording that still implied
  movement/NN zone effects or "biome lakes"; current docs now describe zone
  generation, grass-capacity effects, and zone patches.
- 2026-06-17: Commit remains deferred: the verified tree is interleaved with
  broad pre-existing same-file dirty state and unrelated plan deletions, so
  staging by filename would not faithfully isolate this shipped-plan work.
