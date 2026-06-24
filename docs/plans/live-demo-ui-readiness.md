---
status:        shipped
owner:         unassigned
last_updated:  2026-06-23
okay_to_delete: true
long_lived:    false
owning_docs:
  - architecture/app-shell.md
  - decisions/app-shell.md
---

# Live demo UI readiness

## Mission

Make evosim present well as a short public GitHub Pages / portfolio demo.
The first viewport should foreground the world, keep the right rail compact
and opaque, and put the controls a viewer naturally needs in General: pause /
restart, TPS, population graph, max population, and basic save / import /
export status. Done means a fresh viewer can run and understand the demo
without opening Profiler or hunting through Settings.

## Scope

In scope:

- Right rail visual cleanup: smaller default width, opaque panel/control
  surfaces, denser spacing, and fewer "dev panel" visual leftovers.
- Hamburger / three-line control behavior and visible active state.
- Move or duplicate demo-critical controls into General: TPS, population graph,
  max population, auto-restart, restart, export/import, and autosave/status.
- Keep existing simulation defaults unless a UI default clearly hurts the demo.
- Reorganize Settings categories so advanced knobs remain available but stop
  competing with the live demo controls.

Out of scope:

- Rust simulation behavior, worker loop semantics, SAB layout, or generated TS
  mirrors.
- Replacing the Profiler system.
- Changing saved-world artifact format.
- Large renderer/canvas redesign.
- README screenshots or public copy; that belongs to the follow-up
  README/showcase plan after the UI is visually stable.

## Approach

Implementation sequence: run this after
[`live-demo-reliability-release-gate.md`](live-demo-reliability-release-gate.md)
has fixed the known worker watchdog failure, or treat full e2e as blocked by
that known issue. Do not implement this in parallel with the reliability plan:
both streams can touch `app/web/src/main.ts` and Playwright control specs.

1. Rail shell polish.
   - Likely touched files: `app/web/src/settings.ts`,
     `app/web/src/styles.css`, and possibly `app/web/src/main.ts`.
   - Lower the fresh-user default rail width from the current large value to a
     portfolio-friendly width, likely around `360px` to `384px`, while
     preserving drag resize.
   - Make `#right-rail`, General sections, controls, and tabs read as opaque
     surfaces over the canvas.
   - Keep responsive constraints explicit so narrow viewports do not create
     overlapping tab labels, controls, or chart canvases.

2. General tab cockpit.
   - Likely touched files: `app/web/src/main.ts`,
     `app/web/src/widgets/perf-panel.ts`, `app/web/src/styles.css`, and tests
     that assert control locations.
   - Extract or share existing TPS, max-population, status, and population
     chart logic instead of duplicating independent state.
   - Mount compact demo controls into `#menu-inner` or a dedicated General
     subcomponent.
   - Keep deep profile trees, CPU process monitor, telemetry export, and
     detailed FPS/TPS diagnostics in Profiler.

3. Hamburger behavior.
   - Likely touched files: `app/web/src/main.ts` and
     `app/web/src/rail/index.ts`.
   - Treat the top-bar hamburger as "open / close General".
   - Preserve the intuitive close behavior when General is already open.
   - Make title, `aria-label`, active state, and icon state match the action.
   - Leave the `~` hotkey behavior alone unless implementation reveals a clear
     conflict.

4. Settings organization.
   - Likely touched files: `app/web/index.html`,
     `app/web/src/widgets/devpanel.ts`, and `app/web/src/styles.css`.
   - Keep Settings as the full tuning surface, but demote rarely used and
     render-debug controls below the main demo path.
   - Keep NN as a Settings category unless the product direction changes.
   - Do not hand-edit generated config mirrors; regenerate if codegen-owned
     surfaces change.

5. Documentation migration at ship time.
   - Update `docs/architecture/app-shell.md` to describe the new General /
     Profiler / Settings split.
   - Update `docs/architecture/worker-runtime.md` only if TPS/control
     semantics change.
   - Update `docs/decisions/app-shell.md` if the final layout introduces a new
     rationale worth preserving.

## Exit gate

- `cd app/web && pnpm typecheck`
- `cd app/web && pnpm build`
- Targeted Playwright specs for the moved controls and rail/settings behavior.
- `cd app/web && pnpm test:e2e` after the reliability plan has shipped. If UI
  work is verified before then, report the known worker-watchdog failure from
  `docs/plans/open-bugs.md` separately instead of treating it as a UI failure.
- Manual fresh-state smoke:
  - clear app localStorage;
  - load the app;
  - confirm the rail starts collapsed and the world looks good by default;
  - confirm the hamburger opens compact General and closes it cleanly;
  - confirm General can change TPS and max population;
  - confirm the population graph moves;
  - confirm Profiler still works;
  - confirm Settings Apply / Cancel / Reset still work;
  - confirm no controls overlap on a smaller laptop viewport.
- Ship-time docs migration is complete before setting this plan to `shipped`.

## Open decisions

- Should General show only population, or also FPS/TPS diagnostics?
  Recommendation: show population in General and keep FPS/TPS detail in
  Profiler.
- Should max population move entirely to General or be duplicated in Profiler?
  Recommendation: General owns it; Profiler may reuse the component only if the
  performance workflow still needs it there.
- Should existing users with stored wide rail widths be migrated? Recommendation:
  do not reset stored user layout; fresh portfolio viewers get the new default.

## Implementer brief

Goal:
Make the first-run app UI feel like a polished live demo: compact opaque right
rail, clean hamburger behavior, and General as the demo cockpit.

Non-goals:
Do not retune the sim, rewrite Profiler, change artifact formats, or rebuild
README/assets in this stream.

Authoritative docs:
`docs/architecture/app-shell.md`, `docs/architecture/worker-runtime.md`,
`docs/decisions/app-shell.md`, and this plan.

Likely source areas:
`app/web/index.html`, `app/web/src/main.ts`, `app/web/src/rail/index.ts`,
`app/web/src/widgets/perf-panel.ts`, `app/web/src/widgets/devpanel.ts`,
`app/web/src/settings.ts`, `app/web/src/styles.css`,
`app/web/tests/e2e/settings-rail.spec.ts`, and any e2e files that locate TPS /
Profiler controls.

Expected behavior:
General exposes the controls a viewer needs during a short demo; Profiler
retains deeper performance internals; Settings remains complete but less
visually dominant.

Implementation notes:
Preserve existing DOM IDs where practical, especially `#target-tps-input`, to
avoid unnecessary test churn. If a control moves, update tests to follow the
user-visible flow rather than stale DOM placement.

Cheapest sufficient checks:
`pnpm typecheck`, `pnpm build`, targeted Playwright specs touching rail/settings
behavior, then full `pnpm test:e2e` if IDs or control ownership move.

Stop and report if:
Sharing chart/control code would require a risky Profiler rewrite, or if
General becomes crowded enough that a smaller separate dashboard component is a
better product cut.

## Migration notes

Durable facts migrated to:

- `architecture/app-shell.md`: updated General tab description (TPS, max pop,
  population graph, Restart, auto-restart), Profiler description (FPS/TPS chart
  only, no pop chart), DOM map entries, new Rail geometry section (372 px
  fresh-user default, drag-resize preserved), Settings panel note on shared
  max_population selector.
- `decisions/app-shell.md`: replaced "Settings panel uses a shared navigable
  surface" with "General tab is the demo cockpit" decision covering the new
  cockpit split and the shared perf-panel builder pattern.
- `architecture/worker-runtime.md`: not updated — TPS/control SAB semantics
  unchanged; only the UI surface that exposes TPS moved to General.

## See also

- [`docs/plans/index.md`](index.md)
- [`../architecture/app-shell.md`](../architecture/app-shell.md)
- [`../architecture/worker-runtime.md`](../architecture/worker-runtime.md)
- [`../decisions/app-shell.md`](../decisions/app-shell.md)
