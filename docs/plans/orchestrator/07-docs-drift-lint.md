---
status:        active
owner:         orchestrator
last_updated:  2026-06-12
okay_to_delete: false
long_lived:    false
owning_docs:
  - architecture/testing.md
  - architecture/profiler.md
  - architecture/shared-memory-and-protocol.md
  - architecture/worker-runtime.md
  - architecture/render-pipeline.md
  - agent-context/testing-how-to.md
  - _meta/manifest.md
  - _meta/ownership.json
---

# Docs Drift Fixes and Lint Gate

## Mission

Turn recurring documentation drift into a cheap automated check. Done means the
known drift from the audit is corrected, and a docs-lint command catches broken
anchors, stale code paths, critical constant drift, profiler tree drift, and
ownership/index inconsistencies before they reach review.

## Scope

In scope:

- Triage and fix concrete doc drifts found by the audit and by the first lint
  run.
- Add a repo-root Node docs-lint script exposed through `app/web/package.json`
  and run locally and in CI.
- Check relative markdown links and local code-path anchors.
- Check critical Rust/TS constants and generated mirrors named in the manifest.
- Check profiler tree/span names against code.
- Check docs tree/index/ownership consistency for routed architecture and
  decisions docs.

Out of scope:

- Do not rewrite doc structure broadly; `docs-restructure.md` already handled
  the large structural pass.
- Do not make a natural-language truth checker. The lint should focus on
  mechanical drift with low false-positive risk.
- Do not block all commits on expensive browser e2e just for docs lint.

## Dependencies

Can be Wave 1c. It can run independently from code-heavy plans, but any new
protocol/config plan that lands first should update the lint rules rather than
leaving a known false failure.

## Context Routes

Docs to load:

- `docs/_meta/manifest.md`
- `docs/index.md`
- `docs/architecture/index.md`
- `docs/decisions/index.md`
- `docs/architecture/testing.md`
- `docs/agent-context/testing-how-to.md`
- `docs/_meta/ownership.json`

Code routes:

- `.github/workflows/ci.yml` - CI integration point.
- repo-root docs-lint Node script chosen by the implementer, exposed as
  `pnpm docs:lint` from `app/web/package.json`.
- `app/crates/evosim/src/control_sab.rs`
- `app/crates/evosim/src/constants.rs`
- `app/crates/evosim/src/wasm_api/mod.rs`
- `app/crates/evosim/src/profiler.rs`
- `app/web/src/generated/*`
- `app/web/src/widgets/perf-panel.ts`
- Any existing package/script location chosen by the repo for tooling.

## Workstreams

1. Drift inventory.

   Run a focused manual check against the audit categories: anchors, constants,
   tree lists, and paths. Fix concrete drift first so the lint starts green.
   Treat `Atomics.wait` as the current code truth and stale `waitAsync`
   references as drift to reconcile unless runtime implementation changes that
   fact.

2. Link and path checks.

   Add a markdown scan for relative doc links and local file anchors. It should
   resolve repo-relative code paths and docs-relative links without requiring
   network access.

3. Constants and generated mirrors.

   Encode the manifest's high-risk constants as checks that compare Rust source
   or generated outputs to TypeScript mirrors where appropriate. Prefer parsing
   generated files or using existing binding tests when possible.

4. Profiler and protocol checks.

   Check that profiler spans named in `architecture/profiler.md` exist in code
   and that known top-level tree order stays aligned with `perf-panel.ts`.
   Check SAB constants and control lanes where the generated mirror already
   gives a source of truth.

5. Ownership/index checks.

   Check that architecture and decision docs routed from indexes exist, and
   ownership entries point at existing docs. Avoid requiring indexes to list
   active plans because `docs/plans/index.md` intentionally does not.

6. CI and docs.

   Add the command to CI and to `agent-context/testing-how-to.md`. Decide whether
   it is part of the full drift gate or a lighter doc-only gate.

## Acceptance / Verification

- New docs-lint command passes on the current tree.
- `cd app/web && pnpm docs:lint` is the canonical local command.
- It fails on a deliberately broken relative link, missing routed doc, stale
  critical constant, or missing profiler span in a local smoke/manual test.
- CI runs the command.
- `docs/_meta/manifest.md` drift-gates or drift-verification slot names the new
  command.
- `agent-context/testing-how-to.md` documents how agents run it.
- Expected gates:
  - docs-lint command itself
  - `cd app/web && pnpm typecheck` only if the tool is TypeScript
  - `cargo test --lib` only if Rust binding tests are changed

## Handoff Notes

- Keep the first version intentionally small and reliable. A lint with noisy
  false positives will be ignored.
- Prefer structured checks over regex where the repo already has generated
  mirrors or JSON ownership data.
- Update this plan's owning docs if the lint creates a new canonical tooling
  doc.

## Migration Notes

At ship time, update `manifest.md`, `testing.md`, `testing-how-to.md`, and any
new tooling docs with the final command and what it covers.
