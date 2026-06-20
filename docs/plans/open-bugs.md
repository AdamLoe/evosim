---
status: active
owner: unassigned
last_updated: 2026-06-19
okay_to_delete: false
long_lived: true
owning_docs:
  - architecture/simulation-core.md
  - architecture/worker-runtime.md
---

# Open bugs

Collected from grass-stage3-review.md and v2.0.5/v2.0.x orchestration logs at plans-cleanup (2026-06-05). Each item was either confirmed still-open in the code or could not be confirmed fixed. Fix-confirmed items were dropped (not listed here).

---

## Worker freeze-recovery does not resume ticking (pre-existing)

`worker-watchdog.spec.ts:114` ("simulated unpaused worker freeze is detected and
recovered") fails deterministically: after the stall watchdog detects a frozen
worker and auto-recovers, the replacement worker's tick stays at 1 (expected to
climb by >10 within 5s).

- **Deterministic**, not flaky (reproduced 2/2 with `--repeat-each=2`).
- **Crash recovery** (`worker-watchdog.spec.ts:94`) and the **paused-watchdog**
  test pass; only the *stall*-triggered freeze-recovery path is broken.
- **Pre-existing**, not introduced by the v2.0.0 UX/perf bundle: `sim/worker.ts`
  is unmodified by that bundle and `main.ts`'s `recoverWorker`/`restartWorker`/
  stall-detection are untouched (only rail/canvas wiring changed). Crash recovery
  resets to the same default-slider clean worker and ticks fine, so the bundle's
  constant/default changes are not the cause.
- **Mechanism** (`app/web/src/sim/worker.ts` `freezeForE2E`,
  `app/web/src/main.ts` `checkWorkerWatchdog`/`recoverWorker`):
  a `freeze_after_boot` worker posts `boot_ready` (tick=1) then parks in
  `freezeForE2E()` and never enters `simLoop`. The watchdog should restart it
  into a clean worker that runs `simLoop`, but post-recovery ticking does not
  resume. Needs runtime debugging (logging across the swap, repeated runs).
- **Next step:** a focused `/quick-fix` on the stall-recovery path; do not weaken
  the test.
