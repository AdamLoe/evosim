# Check-docs prompt

You are auditing the `docs/` tree of this repo for drift against the
codebase. Treat every architecture doc's `Code anchors` section and
every embedded constant / contract as a load-bearing claim. Report
mismatches, do not fix them.

For each architecture doc under `docs/architecture/`, do the
following:

1. Open the doc and extract:
   - Every entry in the `Code anchors` section (`path → symbol_name`).
   - Every constant referenced by name in the prose
     (e.g., `MAX_POP_FOR_SIM = 32_000`, `GRASS_CELL_COUNT = 921_600`,
     `NN_INPUTS = 32`, `CREATURE_STRIDE = 8`).
   - Every named function, type, or wasm-bindgen export quoted in the
     interaction-with-neighbours section
     (e.g., `WorldHandle::write_snapshot_to`, `Atomics.waitAsync`).
   - The "Update when" list.

2. For each `path → symbol_name`, verify the symbol exists at the path:
   - Rust files: `grep -nE 'fn|struct|enum|const|impl|mod' <path>`
     should mention the symbol name.
   - TS files: `grep -nE 'function|class|interface|const|export' <path>`
     should mention it.
   - Constant values: `grep -E '(const|pub const)\s+NAME\s*[:=]'` and
     compare the literal value against the doc.
   - **Don't trust line numbers** — none are recorded in the docs.
     Match by symbol name only.

3. For each cross-language constant
   (`MAX_POP_FOR_SIM`, `CREATURE_STRIDE`, `GRASS_BYTES`,
   `SNAPSHOT_HEADER_BYTES`, `CONTROL_SAB_I32_LEN`, the `CTRL_*`
   indices), verify the Rust value matches the TS value, and that the
   doc quotes both (or correctly says "derived from").

4. For every named span (`tick.*`, `frame.*`, `nn.*`, `grass_step.*`)
   referenced in `architecture/profiler.md`, grep the codebase for the
   exact span string. Missing spans (in docs but not in code) or
   orphan spans (in code but not in docs) are both drift.

5. For the worker control path:
   - `web/src/sim-worker.ts → simLoop`: confirm `Atomics.waitAsync`
     is the pacing primitive (not `Atomics.wait`).
   - Confirm the `timeoutMs` line includes `Math.max(1, ...)` — a `0`
     in that floor is the dark-hole regression class.
   - Confirm the not-equal branch awaits a `setTimeout` (macrotask
     yield), not `Promise.resolve()` (microtask).

6. For the build:
   - `Cargo.toml`: `[profile.dev]` and `[profile.release]` both set
     `panic = "abort"`.
   - `.cargo/config.toml`: still contains `--shared-memory`,
     `--max-memory`, `--import-memory`, the TLS exports.
   - `web/vite.config.ts` AND `web/public/_headers`: COOP and COEP
     both set.
   - `web/vite.config.ts`: `worker: { format: "es" }`.
   - `package.json` scripts include `test:e2e`.

7. For every `Update when` bullet across the architecture docs, check
   whether a recent commit touched the named surface without updating
   the doc. Use `git log --oneline -20 -- <path>` to spot-check.

Report shape:

```text
## architecture/<doc>.md
- code anchor `<path> → <symbol>`: PRESENT | MISSING | RENAMED to <new>
- constant `<NAME>`: doc says <docval>, code has <codeval> → OK / DRIFT
- span "<name>": present in code | missing in code | not in docs
- update-when item "<text>": last touched in commit <sha> (<n> days ago);
  doc last touched in <sha> (<n> days ago) → likely stale | OK

## cross-language constants
- MAX_POP_FOR_SIM: Rust=<v>, TS=<v> → OK / DRIFT
- ...

## worker control path
- Atomics.waitAsync vs wait: ...
- timeoutMs floor: ...
- not-equal macrotask yield: ...

## build
- Cargo.toml panic=abort: ...
- ...
```

End the report with a summary of which architecture doc has the most
drift, and which `decisions/<domain>.md` entries are the most likely
candidates for a rewrite.
