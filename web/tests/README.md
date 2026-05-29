# evosim Playwright e2e suite

Smoke tests for the main-thread → sim-worker control path. Exists because
v1.6's `Atomics.waitAsync` loop has now regressed *twice* into a state
where `onmessage` stops firing on the sim worker (sliders / pause / TPS /
inspect / profile all dark-hole) and no unit test would have caught it.

## Run

```bash
cd web
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
- **profile toggle** — toggles `show profiler` + the `#profiler-enable`
  checkbox, asserts all four stacked tables (frame, tick, nn, grass_step)
  populate within 4 s of the click.
- **restart `r`** — presses the `r` hotkey, asserts the tick counter
  resets (drops below the pre-restart value).

Every test forces target-TPS to 1000 *before* interacting, because that is
the regime where `1000/targetTPS - elapsed` clamps to 0 and
`Atomics.waitAsync(.., 0)` returns synchronously. A test at default
TPS=60 passes on the buggy commit — that's how this regression slipped
out of v1.7.

## Adding tests

If you're adding a test for a new main → worker message, do it under
target-TPS=1000 so any future "loop doesn't yield" regression also breaks
your test. Reading state back is easiest via the status bar
(`#status` textContent: `seed: X  ·  tick N  ·  pop P`) or a downstream
observable (e.g. perf-panel render). If you need to read worker state
directly that no UI exposes, plumb it through `request_profile_report`
which already round-trips JSON.
