# Maintaining docs

The doc-authoring **rules** are generic (agent-docs v1) and live in the
global kit:
[`~/.claude/agent-docs/v1/rules/authoring-rules.md`](~/.claude/agent-docs/v1/rules/authoring-rules.md).
Read them when you ship a change that touches a surface in the ownership
map, or when migrating plan context. They cover: explicit ownership,
"describe what IS not what changed," the recoverability test + banned
transcription forms, the no-counts rule, the `decisions/` entry format,
cross-linking, the plan-shipping workflow, and the anti-patterns to
refuse.

This file is a thin in-repo pointer kept at its original path so every
architecture doc's `See also: maintaining-docs` link still resolves.

## The app-specific bits the rules reference

The rules are generic; the data they plug into is this app's, in
[`../_meta/manifest.md`](../_meta/manifest.md):

- **`change-to-doc` slot** — the "if you changed file X → update doc Y"
  table. Consult it before declaring a commit done.
- **`drift-gates` slot** — the per-commit gates the shipping workflow
  runs (full commands in [`testing-how-to.md`](testing-how-to.md)).
- **`code_root` slot** (`app/`) — what doc code-anchors are relative to.
- **`decisions-domains` slot** — this app's `decisions/<domain>.md` set.

Ownership itself is data: [`../_meta/ownership.json`](../_meta/ownership.json)
(see [`../ownership.md`](../ownership.md) for how to use it).

## See also

- [`~/.claude/agent-docs/v1/rules/authoring-rules.md`](~/.claude/agent-docs/v1/rules/authoring-rules.md)
  — the rules.
- [`../_meta/manifest.md`](../_meta/manifest.md) — the app slot-data.
- [`../plans/index.md`](../plans/index.md) — where this app's live plans
  land.
- [`~/.claude/agent-docs/v1/plan-lifecycle.md`](~/.claude/agent-docs/v1/plan-lifecycle.md)
  — plan lifecycle and status metadata (generic).
- [`~/.claude/agent-docs/v1/plan-template.md`](~/.claude/agent-docs/v1/plan-template.md)
  — plan creation skeleton (generic).
- [`coding-style.md`](coding-style.md), [`repo-rules.md`](repo-rules.md).
