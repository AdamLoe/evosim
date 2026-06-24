---
status:        shipped
owner:         unassigned
last_updated:  2026-06-23
okay_to_delete: true
long_lived:    false
owning_docs:
  - architecture/worker-runtime.md
  - architecture/testing.md
  - architecture/build-and-deploy.md
  - decisions/build.md
---

# Live demo reliability and release gate

## Mission

Make the GitHub Pages / portfolio demo trustworthy enough to share. Done means
the known worker stall-recovery bug is fixed without weakening watchdog
coverage, the browser smoke path exercises the same built app shape used for
Pages, and the release checklist proves worker control, shared-memory/threaded
runtime, build/docs drift, and core demo flows.

## Scope

In scope:

- Fix the stall-triggered worker recovery path so a replacement worker resumes
  ticking after an unpaused freeze.
- Preserve `worker-watchdog.spec.ts` coverage, especially the existing
  unpaused freeze-recovery case.
- Add or document a small release smoke for the built Pages-shaped app:
  `/evosim/` base path, boot/tick progress, `crossOriginIsolated`, no
  single-thread collapse, pause/resume, TPS change, restart, Settings/Profiler
  open, and save/export/import path if practical.
- Align CI, deploy, or local release instructions with the checks required for
  a shareable demo.

Out of scope:

- Performance tuning.
- New simulation features.
- Reworking worker architecture beyond what is required for reliable recovery.
- Hand-editing generated TS mirrors unless codegen intentionally changes them.
- README rewrite and screenshots; that belongs after UI cleanup and release
  confidence are established.

## Approach

1. Worker recovery fix.
   - Likely touched files: `app/web/src/main.ts`,
     `app/web/src/sim/worker.ts`, and
     `app/web/tests/e2e/worker-watchdog.spec.ts` only if assertions need to
     become more precise.
   - Start from `checkWorkerWatchdog`, `recoverWorker`, restart/swap logic, and
     the test-only `freeze_after_boot` fault hook.
   - Do not weaken or skip the failing freeze-recovery test.
   - Confirm crash recovery, freeze recovery, and paused no-false-positive
     coverage all still pass.

2. Built-app release smoke.
   - Keep `cd app/web && pnpm test:e2e` as the main worker-control suite.
   - Add a Pages-base built-app smoke if the repo lacks one. Likely shape:
     build with `VITE_BASE=/evosim/`, preview the built `dist`, then have
     Playwright load the base path and assert boot/tick progress plus basic
     controls.
   - If this becomes committed automation, likely touched files are
     `app/web/tests/e2e/*`, `app/web/playwright.config.ts`,
     `app/web/package.json`, and possibly GitHub workflow files.

3. Threaded wasm release proof.
   - Release builds should use `--features threads`.
   - Check the built wasm JS glue still includes thread-pool initialization and
     shared-memory wiring.
   - Keep the COOP/COEP / service-worker shim path documented because GitHub
     Pages differs from Vite dev.

4. Gate/docs alignment.
   - Update `docs/architecture/testing.md`,
     `docs/agent-context/testing-how-to.md`, and
     `docs/architecture/build-and-deploy.md` only if commands or release
     checks change.
   - At ship time, remove or mark resolved the worker stall-recovery entry in
     `docs/plans/open-bugs.md`.

## Exit gate

Required repository gates:

- `cd app && cargo test --lib`
- `cd app && cargo test --lib --features threads`
- `cd app && cargo fmt --all --check`
- `cd app && cargo clippy --all-targets -- -D warnings`
- `cd app && cargo clippy --all-targets --features threads -- -D warnings`
- `cd app && cargo bench --no-run`
- `cd app/web && pnpm docs:lint`
- `cd app/web && pnpm typecheck`
- `cd app/web && pnpm build`
- `cd app/web && pnpm test:e2e`

Release-specific checks:

- `cd app && rustup run nightly wasm-pack build crates/evosim --target web --out-dir ../../web/wasm --release --features threads`
- `grep -c initThreadPool app/web/wasm/evosim.js`
- `grep -F 'shared:true' app/web/wasm/evosim.js`
- `cd app/web && VITE_BASE=/evosim/ pnpm build`
- Built/preview smoke verifies:
  - the app boots under `/evosim/`;
  - `crossOriginIsolated` is true, or an expected documented local-preview
    fallback is recorded;
  - the threaded runtime did not silently collapse to the single-thread path;
  - ticks advance;
  - pause/resume works;
  - TPS changes take effect;
  - restart works;
  - Settings and Profiler open;
  - save/export/import smoke passes if those controls are still visible in the
    final General tab;
  - no fatal console/page errors appear.

## Open decisions

- Should the Pages-base browser smoke be committed as a Playwright project /
  script, or remain a documented manual release checklist?
- Should CI run all Playwright e2e on every PR, or only a smaller smoke while
  the full suite remains local/pre-release?
- Should deploy-pages block on the same browser smoke before upload, or rely on
  CI plus a local release gate?

## Implementer brief

Goal:
Fix the known worker stall-recovery bug and define a release gate that proves
the actual portfolio/GitHub Pages demo path.

Non-goals:
Do not tune performance, add simulation features, rewrite worker architecture,
or touch README/assets.

Authoritative docs:
`docs/plans/open-bugs.md`, `docs/architecture/worker-runtime.md`,
`docs/architecture/testing.md`, `docs/architecture/build-and-deploy.md`,
`docs/agent-context/testing-how-to.md`, and this plan.

Likely source areas:
`app/web/src/main.ts`, `app/web/src/sim/worker.ts`,
`app/web/tests/e2e/worker-watchdog.spec.ts`,
`app/web/playwright.config.ts`, `app/web/package.json`, GitHub workflow files
if release automation changes, and the testing/build docs listed above.

Expected behavior:
After an unpaused worker freeze, recovery swaps in a clean worker that resumes
ticking. The demo build can be verified under the same `/evosim/` base path
used by GitHub Pages.

Implementation notes:
Use the existing fault hook and watchdog tests as the authority. If a new smoke
is added, prefer a small focused built-app test over duplicating the full e2e
suite.

Cheapest sufficient checks:
Targeted `worker-watchdog.spec.ts` while fixing recovery; then full
`pnpm test:e2e`, web build/typecheck/docs lint, and the release-specific smoke.

Stop and report if:
The recovery fix requires changing the worker protocol or SAB layout, or if the
Pages-base smoke cannot model GitHub Pages isolation well enough to be
meaningful.

## Migration notes (filled in at ship time)

No implementation change was required in `main.ts`, `worker.ts`, or the SAB
protocol — the stall-recovery path was already correct. The only change is to
`worker-watchdog.spec.ts`: the freeze-recovery test now uses a single combined
poll condition (`workerState === "running" AND tick > 1`) rather than a
three-step sequence, avoiding the race where W3 boots faster than the Playwright
CDP poll interval. Root cause and resolution recorded in `docs/plans/open-bugs.md`.

Pages-specific release smoke + CI/deploy gating descoped per user (2026-06-23):
GitHub Pages abandoned, Cloudflare Pages planned later; revisit a host-specific
release gate at Cloudflare launch.

Threaded-wasm proof verified: `grep -c initThreadPool app/web/wasm/evosim.js` = 2;
`grep -F 'shared:true' app/web/wasm/evosim.js` = 1. The wasm bundle at HEAD was
already built with `--features threads`; no rebuild was needed.

## See also

- [`open-bugs.md`](open-bugs.md)
- [`../architecture/worker-runtime.md`](../architecture/worker-runtime.md)
- [`../architecture/testing.md`](../architecture/testing.md)
- [`../architecture/build-and-deploy.md`](../architecture/build-and-deploy.md)
