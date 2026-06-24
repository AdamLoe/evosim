---
status: active
owner: unassigned
last_updated: 2026-06-23
okay_to_delete: false
long_lived: true
owning_docs:
  - architecture/simulation-core.md
  - architecture/worker-runtime.md
---

# Open bugs

Collected from grass-stage3-review.md and v2.0.5/v2.0.x orchestration logs at plans-cleanup (2026-06-05). Each item was either confirmed still-open in the code or could not be confirmed fixed. Fix-confirmed items were dropped (not listed here).

---

## ~~Worker freeze-recovery does not resume ticking (pre-existing)~~ RESOLVED 2026-06-23

**Resolved.** Root cause was a test-design race, not an implementation defect in
`main.ts` or the worker. The stall-recovery path itself was always correct; the
test's assertion window was too narrow and started measuring from when the
*frozen* worker (W2) booted rather than from when the *replacement* worker (W3)
actually began running.

**Root cause (test):** `simulateWorkerFreeze` spawns W2 with `freeze_after_boot`.
W2 boots (tick=1), posts `boot_ready`, and parks in `Atomics.wait`. The old test
then polled for `workerState = "running"` (catches W2's boot), read
`recoveredTick = 1`, and polled for `tick > recoveredTick + 10` within 5 s.
But W2 is frozen for the full `WORKER_STALL_TIMEOUT_MS` (3.5 s) before the
watchdog fires and W3 boots. By the time W3 is running and tick advances, the 5 s
window had expired.

**Second race:** `"stalled"` → `"restarting"` is a synchronous transition in the
same microtask (both `setWorkerState` calls happen before the first `await` in
`recoverWorker` / `spawnTrackedWorker`). A separate `not.toBe("running")` poll
can be skipped entirely when W3 boots faster than the Playwright CDP poll interval
(~100 ms); in that case the poll only ever sees `"running"` for both W2 and W3.

**Fix (worker-watchdog.spec.ts):** Replace the three-step poll sequence with a
single combined condition: `workerState === "running" AND tick > 1`. Since W2 is
frozen at tick=1 forever, this condition is only true once W3's `simLoop` has
advanced the tick counter. The 15 s timeout covers the full cycle (W2 boot ~0.5 s
+ stall window 3.5 s + W3 boot ~0.5 s + margin). Passes 10/10 with
`--repeat-each=10`.

**No implementation change was required** to `main.ts`, `worker.ts`, or the SAB
protocol. The recovery path was sound.
