---
status:        shipped
owner:         unassigned
last_updated:  2026-06-17
okay_to_delete: true
long_lived:    false
owning_docs:
  - architecture/biome.md
  - architecture/simulation-core.md
  - architecture/render-pipeline.md
  - architecture/app-shell.md
  - decisions/sim.md
---

# Grass/no-grass zone toggle

## Mission

Replace the user-facing "desert/ocean/water biome" framing with a simpler
grass-vs-no-grass-zone model, and add a next-world setting that disables the
simulation effect entirely. Done means a user can turn off no-grass zones for a
new world, the sim then uses normal grass capacity everywhere, and the visual
zone layer no longer appears for that run.

## Scope

In scope:

- Add a construction/next-world setting that enables or disables no-grass zones.
  This is expected to require restart because zone generation and per-cell grass
  capacity are built with world dimensions at construction.
- When disabled, construct the grass capacity map as all normal grass capacity
  and publish/render no dead-zone visual effect for that world.
- Rename user-facing UI labels, docs, and comments that describe this feature as
  desert, ocean, water, or biome terrain when the current product concept is
  simply grass vs no-grass.
- Keep the existing Display-only world/zone opacity behavior, but make sure it
  cannot show dead zones when the simulation effect is disabled.
- Update saved-world/world-config handling if the new setting must persist with
  world artifacts.

Out of scope:

- Reintroducing movement penalties, energy penalties, or direct biome NN inputs.
- Adding separate water/desert/ocean behavior. The target model is binary:
  normal grass capacity vs no-grass/dead-grass capacity.
- Making the toggle live during a run unless the implementer proves that
  rebuilding capacity, snapshot metadata, and visuals can be done safely without
  world reconstruction.
- Retuning creature equilibrium beyond what is required to keep the default
  world alive with zones enabled and disabled.

## Context Routes

- `docs/architecture/biome.md` for the current static zone generation and grass
  capacity effect.
- `docs/architecture/simulation-core.md` for `WorldConfig`, construction-only
  settings, grass capacity ownership, and saved-world state.
- `docs/architecture/render-pipeline.md` for the current terrain/zone texture
  drawn under grass.
- `docs/architecture/app-shell.md` for Settings staged construction controls.
- `docs/decisions/sim.md` for rationale that must be renamed or revised when
  the "biome" concept becomes grass/no-grass.
- Code routes: `app/crates/evosim/src/world/biome.rs`,
  `app/crates/evosim/src/grass/mod.rs`, `app/crates/evosim/src/world/mod.rs`,
  `app/crates/evosim/src/wasm_api/mod.rs`, `app/web/src/settings.ts`,
  `app/web/src/widgets/devpanel.ts`, and `app/web/src/render/gl.ts`.

## Open Assumptions

- The requested disable switch is a simulation switch, not just visual opacity.
- Disabling the simulation effect should also remove the visual zone effect for
  that world.
- The feature can stay construction-only because zone generation, capacity
  arrays, and world artifact state are construction-scoped today.
- Internal enum names may remain temporarily if renaming them would dominate the
  change, but user-facing labels and canonical docs should describe the feature
  as grass/no-grass zones, not desert/ocean/water terrain.

## Approach

Plan this as a model-language cleanup plus a construction setting. The core
implementation should thread a single setting through `WorldConfig` and world
construction, then use it to choose between generated no-grass zones and an
all-normal-capacity field. The renderer should receive a zone channel that is
plain/empty when the effect is disabled, so visual opacity settings cannot
accidentally reintroduce a disabled simulation concept.

Avoid broad ecosystem/biome expansion. The user request is explicitly to narrow
the concept, so implementation and docs should remove stale references rather
than inventing more terrain types.

## Acceptance / Verification

- Settings exposes a next-world no-grass-zone enable/disable control with clear
  restart semantics.
- With zones enabled, existing default behavior is preserved aside from updated
  labels/docs.
- With zones disabled and a restarted/fresh world, grass can grow to normal
  capacity everywhere and the rendered zone layer shows no dead-zone pattern.
- Saved/resumed worlds preserve the chosen zone setting or otherwise document a
  deliberate migration fallback.
- Tests cover both enabled and disabled construction paths for capacity behavior,
  and a targeted UI/persistence smoke covers the new setting if web settings are
  touched.
- Architecture and decision docs no longer present the feature as desert vs
  ocean/water; they describe the current grass/no-grass-zone model.

## Handoff Notes

This work touches Rust construction, renderer interpretation, settings, and
docs. Keep it separate from render LOD work except for the shared Display area
in Settings. Review should focus on whether disabling the simulation effect also
removes the visual effect, and whether old "biome" references remain in
canonical docs or user-facing UI.

## Migration Notes

When this ships, migrate current-state facts into `architecture/biome.md`,
`architecture/simulation-core.md`, `architecture/render-pipeline.md`, and
`architecture/app-shell.md`. Update `decisions/sim.md` so the durable rationale
uses grass/no-grass terminology and explains why the toggle is construction-only
if that remains true.

Shipped 2026-06-17:

- Implemented `WorldConfig.grass.no_grass_zones` and Settings `noGrassZones` as
  a next-world construction toggle.
- Disabled worlds construct all-plains zone bytes, normal capacity everywhere,
  and no dead-zone visual pattern.
- Migrated durable facts to `architecture/biome.md`,
  `architecture/simulation-core.md`, `architecture/render-pipeline.md`, and
  `architecture/app-shell.md`; rationale lives in `decisions/sim.md`.
