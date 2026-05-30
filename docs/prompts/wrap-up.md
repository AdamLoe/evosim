# Wrap-up prompt

You are closing out this chat. Land everything in a safe, resumable state
before the conversation ends. Work through the steps in order — do not
skip ahead, and do not bundle steps that need user confirmation into a
single tool call.

## Order of operations

1. **Plans.** Any in-flight work from this chat that is not already
   tracked in `docs/plans/` must land in a plan. Decide per piece of
   work:
   - If an existing plan already covers it, update that plan (refresh
     `last_updated`, adjust `status` if it changed, add new
     checklist/scope items as needed).
   - If nothing covers it, create a new plan from
     [`../plans/template.md`](../plans/template.md). Follow the
     lifecycle rules in [`../plans/index.md`](../plans/index.md) —
     frontmatter is mandatory, prose lifecycle boilerplate is not.
   - If the work is trivial (one-line fix, no follow-up) and obviously
     captured by the git diff alone, no plan entry is needed. Say so
     out loud rather than silently skipping.

   **Plans are scratch and get deleted post-ship.** A plan never
   substitutes for a current-state doc update. After step 1, you owe
   step 2 — for every plan touched here, the plan's `owning_docs:` is
   your literal checklist of files to inspect in step 2.

2. **Docs.** Update `docs/` for anything shipped this chat that the
   docs don't yet reflect. **Start from the `owning_docs:` list on
   every plan you touched in step 1 — open each one and verify it
   describes the current state.** Keyword grep is not sufficient;
   read the relevant sections. **Be conservative.** The rules in
   [`../agent-context/maintaining-docs.md`](../agent-context/maintaining-docs.md)
   apply in full — in particular:
   - Architecture docs describe what **IS**. No "this chat added…",
     no version-flavoured framing. Rewrite in place.
   - Don't touch `decisions/<domain>.md` unless this chat made a real
     design choice worth recording (mandatory fields: `Decision`,
     `Why`, `Applies to`). Refactors, bug fixes, and follow-throughs
     on existing decisions are **not** new decisions.
   - Use the trigger table in `maintaining-docs.md` to decide which
     architecture doc owns each touched surface. If nothing in the
     table fires, the docs are already correct — don't invent edits.
   - Don't add prompts, agent-context entries, or repository-layout
     rows unless this chat actually changed that surface.

   If you're unsure whether a change warrants a doc update, state the
   change in one line and ask the user before editing.

3. **Commit.** Bundle plan updates, doc updates, and any remaining
   uncommitted source changes from this chat into a single commit on
   the current branch. Follow the per-commit gates in
   [`../agent-context/testing-how-to.md`](../agent-context/testing-how-to.md)
   and the commit-message style of recent commits (`git log --oneline -10`).
   Do not push. Do not amend.

4. **Worktrees.** Run `git worktree list`. For each worktree other
   than the primary checkout:
   - If the chat context makes clear what the pending changes are
     (e.g., you spawned a subagent with `isolation: "worktree"` and
     know what it was doing), commit them on the worktree's branch
     with a sensible message, then `git worktree remove` it.
   - If the pending changes are ambiguous, ask the user what to do
     before committing or removing. Never discard work you can't
     explain.
   - Goal: end with only the primary worktree remaining.

5. **Background agents and processes.** Stop anything still running:
   - Any background `Agent` invocations from this chat — send a
     `TaskStop` (or equivalent) so they don't keep burning context.
   - Any background `Bash` processes (dev server, watchers,
     long-running tests) started during this chat that the user did
     not explicitly want to leave running. Kill them.
   - Report what was stopped.

## Final report

End with a short status block the user can read at a glance:

```
plans:      <created N / updated M / none>
docs:       <files touched, or "no updates warranted">
commit:     <sha + subject>
worktrees:  <removed N, kept primary>
processes:  <stopped: list, or none>
```

Then stop. Do not start new work after the wrap-up report.