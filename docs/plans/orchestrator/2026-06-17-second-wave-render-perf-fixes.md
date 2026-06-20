# Second-wave render/perf fixes hub

Created: 2026-06-17

## Request

Follow-up fixes based on the repeat-camera/LOD/TPS work:

- Add a slider for max zoom out and increase the default. The slider should
  have a high min and max.
- Fix the map border disappearing after LOD settings were added.
- Investigate why visible seams remain despite no border, and fix concrete
  frontend rendering problems found.
- Confirm whether repeating the map increased backend requirements. It should
  be frontend reuse only and should not affect backend work.
- Implement concrete fixes found from TPS stutter diagnosis where safe.

## Lifecycle

Briefed implementation with review. The work is user-facing and spans render
settings, WebGL repeat rendering, and performance behavior, but it is a
follow-up to already-shipped plans rather than a new tracked plan.

## Initial state

The worktree is already broadly dirty from the prior implementation and
pre-existing user/in-progress edits. Preserve existing changes. Do not reset,
checkout, or revert unrelated files.

## Streams

| Stream | Area | Status | Last observed fact | Next action | Blockers |
|---|---|---|---|---|---|
| Implement second wave | Render/settings/perf/docs | complete | Max zoom-out setting, border buffer fix, repeat terrain shader seam fix, tests/docs updated | Close out | same-file dirty tree prevents safe commit |
| Review second wave | Final review/verification | complete | Review fixed one-axis repeat seam case and verified second-wave scope | Report closeout | same-file dirty tree prevents safe commit |

## Decisions

- Use one implementation worker because the likely write set overlaps
  `app/web/src/main.ts`, `app/web/src/render/*`, `app/web/src/settings.ts`,
  `app/web/src/widgets/devpanel.ts`, e2e tests, and render/perf docs.
- Repeating the map should stay frontend-only. Backend or shared protocol
  changes are allowed only if the worker finds and explains a real existing
  coupling bug.
- TPS fixes should be concrete and bounded, such as correcting control-path
  live-apply or render-loop pacing issues. Avoid broad simulation algorithm
  rewrites in this follow-up.

## Evidence Log

- 2026-06-17: Intake loaded manifest, index, overview, orchestration rules,
  and plan lifecycle.
- 2026-06-17: Dispatched implementation worker `Wegener`
  (`019ed3e2-c716-7fa3-b652-ae0bd00a260f`) for zoom slider, border/seam
  fixes, backend-impact check, and bounded TPS fixes.
- 2026-06-17: Added live Display `maxZoomOutMaps` slider (4..64 maps,
  default 8) and routed camera clamp, wheel/pinch, initial camera, and worker
  camera-lane seeding through it.
- 2026-06-17: Border root cause was frontend GL state: frame buffer storage was
  8 floats while the rectangle border upload is 48 floats. Fixed allocation to
  match `FRAME_VERTS`; no backend change.
- 2026-06-17: Seam root cause was not border geometry. Repeated full-world
  terrain reused the base clipmap correctly, but grass procedural wave/noise
  used non-periodic raw coordinates at tile edges. Shader now uses tile-periodic
  UVs for repeat-period grass detail.
- 2026-06-17: Backend impact confirmed frontend-only. Repeat rendering reuses
  the same snapshot and creature/grass/biome windows; main folds camera lanes
  into the base world for worker clipmap selection. No larger backend snapshot
  or sim protocol requirement found.
- 2026-06-17: TPS diagnosis rechecked. Target TPS already live-applies through
  `CTRL_TARGET_TPS_BITS` + futex wake; shipped diagnosis points to tick-cost
  saturation at high targets. No safe bounded pacing code change made in this
  wave.
- 2026-06-17: Dispatched review worker `Dewey`
  (`019ed3ed-3bdd-7702-b1df-dbee696b94b8`) to verify implementation,
  docs/tests, backend-impact answer, TPS conclusion, and commit safety.
- 2026-06-17: Review found one seam gap: procedural grass switched to
  tile-periodic detail only when both repeat axes wrapped, leaving one-axis
  repeat views exposed. Fixed `render/gl.ts` so any repeating axis uses the
  periodic terrain-detail path. Code review otherwise confirmed the
  `maxZoomOutMaps` live Display slider/default/tests, the frame buffer
  allocation matching `FRAME_VERTS`, frontend-only camera folding, and the
  target-TPS tick-cost conclusion.
- 2026-06-17: Review verification passed `cd app/web && pnpm typecheck`,
  `cd app/web && pnpm docs:lint`, and focused Playwright
  `render-camera.spec.ts`, `defaults-drift.spec.ts`, and
  `settings-persistence.spec.ts` against a temporary dev server on port
  47822. The default port 47821 was already occupied by a production-bundle
  server, which made the first settings-persistence run miss DEV-only hooks.
- 2026-06-17: Commit safety blocked: second-wave files share the dirty tree
  with unrelated/prior edits, including same-file changes in frontend render
  and settings modules. Staging whole files would include unrelated work, so no
  review commit was created.
