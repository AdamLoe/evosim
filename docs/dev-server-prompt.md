# Dev Server Management Guide

## 1. What this is

evosim is a Rust→WebAssembly browser simulation. The Rust crate is compiled
to wasm via `wasm-pack` and the output lives in `web/wasm/`. The frontend
(TypeScript + Vite) in `web/` imports that wasm and renders the simulation in
the browser. The dev server is Vite running at **port 47821** (pinned with
`strictPort: true`); it serves `web/` with hot-module reload and injects the
COOP/COEP headers that SharedArrayBuffer (rayon threads) requires.

---

## 2. How to check if it is already running

```bash
# Prints the PID if Vite is running on 47821, nothing if it is not.
pgrep -f 'vite.*47821'

# HTTP liveness check.
curl -sf http://localhost:47821/ -o /dev/null && echo "up" || echo "down"
```

---

## 3. How to start it

> ## 🟢 ALWAYS USE THIS COMMAND
>
> ```bash
> cd /home/adamg/evosim && \
>   rustup run nightly wasm-pack build --target web --out-dir web/wasm --dev --features threads
> ```
>
> **Use the threaded build by default. No exceptions.** The plain
> `wasm-pack build --target web --out-dir web/wasm --dev` (without
> `--features threads`) silently produces a single-threaded bundle:
> the rayon pool is never initialized, `rayon::current_num_threads()`
> returns 1, and the entire NN forward pass runs on the main thread
> with no error or warning. The sim *looks* fine — just runs at ~1/8
> speed on a typical desktop CPU.
>
> Every required compile + link flag for atomics, shared memory,
> imported memory, and the wasm-bindgen-rayon TLS exports lives in
> `.cargo/config.toml` under `[target.wasm32-unknown-unknown]`. Don't
> re-pass them via `RUSTFLAGS` env vars — that *overrides* the config
> and breaks the build silently.
>
> **Always rebuild wasm after any Rust change.** `web/wasm/` is gitignored
> and not auto-rebuilt by `pnpm dev`. If you skip this, the TS layer will
> hit ABI errors against a stale bundle (e.g. `stride !== 6` if a recent
> commit changed `creature_stride()`).
>
> **Verify the bundle is threaded** after every build:
>
> ```bash
> grep -c initThreadPool web/wasm/evosim.js   # → 2 = threaded, 0 = plain
> grep -F 'shared:true'  web/wasm/evosim.js   # → one hit = shared memory wired
> ```
>
> If `initThreadPool` count is 0, you forgot `--features threads`. If
> `shared:true` is missing, the linker didn't emit shared memory — check
> `.cargo/config.toml` hasn't been clobbered.
>
> **Confirm at runtime** by opening DevTools console after the page loads.
> You want this line:
>
> ```
> [threads] pool ready: hardwareConcurrency=N crossOriginIsolated=true
> ```
>
> If you see `[threads] not spawned. SharedArrayBuffer=false ...` →
> COOP/COEP headers aren't reaching the doc. If you see
> `[threads] initThreadPool failed; continuing single-threaded: ...` →
> the wasm bundle is missing some thread requirement.
>
> See section 7 for the plain (non-threaded) variant — only useful for
> quick build-only iteration where you don't actually run the sim.

```bash
cd /home/adamg/evosim/web && rm -f /tmp/vite.log && \
nohup setsid bash -c 'PATH=$HOME/.local/bin:$PATH pnpm dev --host 0.0.0.0' \
  > /tmp/vite.log 2>&1 < /dev/null &
disown
```

Wait a few seconds, then verify:

```bash
sleep 4 && curl -sf http://localhost:47821/ -o /dev/null && echo "OK"
```

Also confirm the wasm JS glue is served:

```bash
curl -sf http://localhost:47821/wasm/evosim.js -o /dev/null && echo "wasm OK"
```

---

## 4. How to stop it

```bash
# If you have the PID (from pgrep above):
kill <PID>

# Or by pattern:
pkill -f 'vite.*47821'
```

After stopping, `pgrep -f 'vite.*47821'` should print nothing.

---

## 5. How to restart cleanly

```bash
# Stop
pkill -f 'vite.*47821'
sleep 1

# Start (same one-liner as section 3)
cd /home/adamg/evosim/web && rm -f /tmp/vite.log && \
nohup setsid bash -c 'PATH=$HOME/.local/bin:$PATH pnpm dev --host 0.0.0.0' \
  > /tmp/vite.log 2>&1 < /dev/null &
disown

sleep 4 && curl -sf http://localhost:47821/ -o /dev/null && echo "OK"
```

---

## 6. How the user opens it in a browser

| Context | URL |
|---------|-----|
| Inside WSL2 (curl/wget) | `http://localhost:47821/` |
| Windows browser (WSL2→Windows) | Use the `172.x.x.x:47821` line that Vite prints in `/tmp/vite.log` |

To find the Windows-reachable address:

```bash
grep '172\.' /tmp/vite.log
# Example output:  ➜  Network: http://172.26.11.236:47821/
```

Open that URL in your Windows browser.

---

## 7. Common issues

### Port already in use

`strictPort: true` means Vite will **refuse to start** (hard error) rather
than silently pick a different port.

```bash
# Find what is holding the port:
lsof -i :47821

# Kill the offender by PID, then retry.
kill <PID>
```

### Wasm not built / out of sync

The wasm bundle in `web/wasm/` is gitignored and **not auto-rebuilt by
`pnpm dev`**. Any Rust change requires a manual rebuild. Symptoms of a
stale bundle:

- Browser console: fetch error for `evosim_bg.wasm`, or the file is missing
- Runtime crash with `stride !== N` or "X is not a function" type errors
  when TS calls a wasm-bindgen export whose signature changed
- `pnpm typecheck` errors against `web/wasm/evosim.d.ts` after Rust edits

Rebuild from the repo root:

```bash
# Threaded dev build — THIS IS THE DEFAULT, USE THIS COMMAND.
# Compile + link flags for atomics, shared memory, imported memory, and
# wasm-bindgen-rayon TLS exports all live in .cargo/config.toml.
cd /home/adamg/evosim && \
  rustup run nightly wasm-pack build --target web --out-dir web/wasm --dev --features threads

# Threaded release build — only for perf measurements or shipping.
cd /home/adamg/evosim && \
  rustup run nightly wasm-pack build --target web --out-dir web/wasm --release --features threads

# Plain (non-threaded) build — NOT FOR RUNNING THE SIM. Rayon falls back to
# a single thread, the NN forward pass runs on the main thread, and you'll
# see ~1/Nx the throughput you'd get from threads. Only use this if you
# specifically want to test the cfg(not(feature = "threads")) code path.
cd /home/adamg/evosim && wasm-pack build --target web --out-dir web/wasm --dev
```

**Verify which build you ended up with:**

```bash
grep -c initThreadPool web/wasm/evosim.js   # 2 = threaded, 0 = plain
grep -F 'shared:true'  web/wasm/evosim.js   # one hit = shared memory wired
```

A silent fall-through to the plain build is the most common footgun — the
sim still runs but `rayon::current_num_threads()` returns 1 and the
parallel NN pass collapses to a single chunk. **Always check the grep
output after every Rust change.**

Then hard-reload the browser (Ctrl+Shift+R). Vite's HMR picks up the new
`.js` glue automatically; the `.wasm` is fetched fresh on reload. Restart
the dev server (section 5) only if HMR doesn't pick it up.

### pnpm not on PATH

pnpm is installed at `~/.local/bin/pnpm`. The start command already prepends
that directory, but if you run pnpm manually:

```bash
export PATH=$HOME/.local/bin:$PATH
# or use the full path:
~/.local/bin/pnpm dev --host 0.0.0.0
```

### COOP/COEP headers (SharedArrayBuffer / rayon)

The headers `Cross-Origin-Opener-Policy: same-origin` and
`Cross-Origin-Embedder-Policy: require-corp` are set in `web/vite.config.ts`
for both `server` and `preview`. If the browser console complains about
`SharedArrayBuffer` not being available, that is a config drift bug — check
that those headers are still present in `vite.config.ts`. This should not
happen under normal operation.

### Vite cache stale

Very rare. If you see inexplicable module resolution errors:

```bash
rm -rf /home/adamg/evosim/web/node_modules/.vite
# Then restart the server normally.
```

---

## 8. What to do if the user reports a UI bug

1. Open `http://localhost:47821/` (or the 172.x.x.x address from section 6)
   and reproduce the bug.
2. Open browser DevTools → Console for JS errors; Network tab for failed
   fetches.
3. Navigate to the relevant source file:

| Symptom | File(s) to look at |
|---------|-------------------|
| Canvas / rendering wrong | `web/src/render.ts` |
| Speed controls, RAF loop, boot | `web/src/main.ts` |
| Rail layout / tabs | `web/src/rail/index.ts` |
| Inspector contents | `web/src/rail/inspector.ts` |
| Pop chart | `web/src/rail/stats.ts` |
| Dev panel sliders | `web/src/widgets/devpanel.ts` |
| Perf panel | `web/src/widgets/perf-panel.ts` |
| Simulation logic (Rust) | `src/world/tick.rs`, `src/world/mod.rs` |
| NN forward pass / weight layout | `src/brain.rs` |
| NN inputs / proximity sensors | `src/world/proximity.rs` + `src/world/nn.rs::build_nn_input` |
| Grass mechanic | `src/grass.rs` |
| Wasm API surface | `src/wasm_api.rs` (`WorldHandle`) |

Hot-reload is active: edit a `.ts` file and the browser refreshes
automatically. Rust changes require a `wasm-pack build …` (see section 7)
followed by a hard browser reload. If a Rust commit changed a wasm-bindgen
signature (e.g. added/removed/renamed a `WorldHandle` method, or changed
`creature_stride()`), you MUST rebuild — `pnpm dev` will happily serve a
stale wasm against new TS code, producing confusing runtime errors.

### v1.3 deletions (no longer in the codebase)

The following surfaces were removed in v1.3 and have NO source files —
don't go looking for them:

- `web/src/eulogy/*`, `src/hof.rs` — Hall of Fame + eulogy modal (D5)
- `web/src/persistence/*`, `src/save.rs` — save/load (D1); reload = fresh world
- Toast stack — `web/src/rail/toast.ts` (D4)
- Species panel / chart, `src/species.rs` (D10)
- `src/carrion.rs`, scavenge action (D2)
- `src/genome.rs` — creatures are now structurally identical (D3)
- `src/torus.rs` — world is walled (D7)
- `src/snapshot_hash.rs` + `tests/acceptance.rs` + goldens (D6)

### v1.5 deletions (no longer in the codebase)

- `src/vision.rs` — 24-sector RGB raycast vision (S5b); replaced by
  `src/world/proximity.rs` with semantic 8-sector proximity inputs.
- `CreatureSoA::eye_trig`, `CreatureSoA::last2_action`, `CreatureSoA::prev_energy`
  — dead SoA columns (S5b).
- `Brain::color_rgb`, `Brain::weight_hash`, `hash_weights`, `color_from_hash`
  — NN-weight-hash color replaced by per-creature action-EMA (S3).
- `World::peak_population`, `first_move_fired`, `first_eat_fired`,
  `population_milestones_fired` — vestigial since D4 (S5b).
- `creatures_buffer` `flag_eye/move/mouth/armor` slots — stride dropped
  10→6, render now reads `[x, y, radius, r, g, b]`.

---

## 9. Tail recent server logs

```bash
tail -50 /tmp/vite.log
```

Full log since last start is always at `/tmp/vite.log`. The file is wiped on
each `rm -f /tmp/vite.log` in the start command (section 3), so it only
contains output from the current run.
