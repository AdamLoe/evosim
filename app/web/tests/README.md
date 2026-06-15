# evosim Playwright e2e suite

Smoke tests for the main-thread → sim-worker control path. Exists because
the worker loop can regress into a state where `onmessage` stops firing on
the sim worker (sliders / pause / TPS / inspect / profile all dark-hole)
and no unit test would catch it.

## Run

```bash
cd app/web
pnpm install            # one-time
pnpm test:e2e
```

The runner uses `webServer` to boot the Vite dev server itself, so you do
not need to start one separately. If a dev server is already running on
`:47821` it is reused (`reuseExistingServer: true`).

If chromium is not installed:

```bash
npx playwright install chromium
```

The container already has `~/.cache/ms-playwright/chromium-1223` so this
should not be needed on the dev box.

## Headed / debug

```bash
pnpm test:e2e --headed              # watch it run
pnpm test:e2e --debug                # step through with the inspector
pnpm test:e2e --project chromium --grep "pause"   # one test at a time
```

## What's covered

`tests/e2e/sim-bridge.spec.ts`:

- **pause + resume** — clicks `#playpause-btn`, asserts the tick counter
  stops within ~300 ms and resumes after a second click.
- **target TPS** — selects values in the `#target-tps-input` `<select>`
  and asserts observed ticks/s tracks the dropdown.
- **slider change** — opens the dev-panel, edits the `basic upkeep`
  numeric input, asserts the worker did not log a `set_slider … rejected`
  warning (which the bridge wraps wasm errors with).
- **profile toggle** — opens the Settings rail's Profiler category and
  asserts the profiler trees populate within 4 s.
- **restart `r`** — presses the `r` hotkey, asserts the tick counter
  resets (drops below the pre-restart value).

Every test forces target-TPS to 1000 *before* interacting, because that is
the regime where futex wake handling, pacing overshoot, and snapshot
back-pressure failures surface. A test at default TPS=60 can pass while
high-throughput control is broken.

## Adding tests

If you're adding a test for a new main → worker message, do it under
target-TPS=1000 so any future "loop doesn't yield" regression also breaks
your test. Reading state back is easiest via the status line
(`#perf-status-line` textContent: `seed: X · tick N · pop P`) or a downstream
observable (e.g. profiler-tree render). If you need to read worker state
directly that no UI exposes, plumb it through `request_profile_report`
which already round-trips JSON.
