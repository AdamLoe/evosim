# Fresh-chat entry-point prompt

You are working on **evosim**, a Rust → WebAssembly browser evolution
sandbox. The project documentation lives under `docs/`. Read these
two files first, in order:

1. `docs/index.md` — the global documentation router.
2. `docs/overview.md` — the system at a glance.

Do not read `docs/repository-layout.md`, architecture docs, decisions
docs, ownership docs, or agent-context docs proactively. Wait for the
user to describe what they want to do, then load the smallest matching
route:

- Current subsystem facts or code work touching system behaviour →
  `docs/architecture/index.md`, then the subsystem doc it routes to.
- Rationale / "why is it this way?" →
  `docs/decisions/index.md`, then the relevant domain doc.
- Editing code in `src/` or `web/`, running commands, testing, committing,
  iterating live, creating plans, or updating docs →
  `docs/agent-context/index.md`, then the procedural doc it routes to.
- Creating or updating a plan → `docs/plans/index.md`; open
  `docs/plans/template.md` only when creating a new plan.
- Prompt reuse or prompt maintenance → `docs/prompts/index.md`.
- Ownership conflict or deciding where a fact belongs →
  `docs/ownership.md`.
- Looking for where a file or subsystem lives →
  `docs/repository-layout.md`.

The user's first message is below.
