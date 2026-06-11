# Documentation ownership

The canonical owner map — "which one doc owns this concept" — is now
**structured data**, not a prose table. It lives at
[`_meta/ownership.json`](_meta/ownership.json) (agent-docs v1, Layer A:
concept → owner doc + allowed referencers; no code anchors).

## How to use it

- **Finding an owner:** read `_meta/ownership.json` and match your
  concept. The `owner` field is the one doc you edit; every other doc
  *links* to it instead of redefining the concept.
- **The rule itself** ("edit the owner; non-owners link, never redefine;
  move drift back to the owner") is generic and lives in the global
  authoring rules (`~/agent-docs/v1/rules/authoring-rules.md`,
  rule 1), not here.
- **Adding/maintaining owners:** when a concept gets a new canonical
  owner or a cross-doc conflict appears, add/edit an entry in
  `_meta/ownership.json`.

This file is a thin pointer kept so existing `See also` links resolve.
The data is the JSON; the rules are global.

## See also

- [`_meta/ownership.json`](_meta/ownership.json) — the owner map (data).
- [`index.md`](index.md) — global docs router.
- [`architecture/index.md`](architecture/index.md) — what these own.
