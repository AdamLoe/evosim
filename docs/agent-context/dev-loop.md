# Dev loop

The inner loop: rebuild wasm, restart the server, when to clear caches.

## When does this apply

Any time you're iterating on a Rust or TS change and want to see it run
in the browser. The canonical build commands, flags, and threaded-bundle
invariants live in
[`../architecture/build-and-deploy.md`](../architecture/build-and-deploy.md);
this doc is the procedural version for an agent at the keyboard. It is also the canonical landing for the dev-server workflow (the
`docs/prompts/dev-server.md` prompt file has been deleted; this doc
replaces it).

## The inner loop

For a full local rebuild-and-serve pass with no HMR/watch mode, run:

```bash
# Run from app/ (the workspace root)
./local_dev.sh
```

The script builds native Rust targets with and without `threads`, regenerates
the TypeScript mirrors, removes stale `web/dist` + `web/wasm`, rebuilds the
threaded wasm bundle, runs `pnpm build`, and serves the built app with
`pnpm preview` on port 47821. Stop it with Ctrl-C and rerun the same command
after agent changes to rebuild from a clean web bundle.

1. **Rust change?** Rebuild wasm. Always with `--features threads`. No
   exceptions:

   ```bash
   # Run from app/ (the workspace root)
   rustup run nightly wasm-pack build crates/evosim --target web --out-dir ../../web/wasm --dev --features threads
   ```

   Verify the bundle is actually threaded:

   ```bash
   grep -c initThreadPool web/wasm/evosim.js   # → 2 (threaded) | 0 (plain)
   grep -F 'shared:true'  web/wasm/evosim.js   # → 1 hit
   ```

   If `initThreadPool` count is 0, you forgot `--features threads`. Run
   it again. See
   [`../architecture/build-and-deploy.md`](../architecture/build-and-deploy.md)
   for every flag's purpose and the full build/deploy contract.

2. **TS change?** No build step needed — Vite HMR picks it up. But you
   may need to hard-reload (Ctrl+Shift+R) if the change touches the
   worker boot path.

3. **Start the dev server (or check it's running).**

   ```bash
   pgrep -f 'vite.*47821'                                 # PID or empty
   curl -sf http://localhost:47821/ -o /dev/null && echo up || echo down
   ```

   If down:

   ```bash
   # From app/ workspace root, cd into web/ for pnpm
   cd web && rm -f /tmp/vite.log && \
     nohup setsid bash -c 'PATH=$HOME/.local/bin:$PATH pnpm dev --host 0.0.0.0' \
     > /tmp/vite.log 2>&1 < /dev/null &
   disown
   sleep 4 && curl -sf http://localhost:47821/ -o /dev/null && echo OK
   ```

4. **Hard-reload the page** if you just rebuilt wasm. Vite's HMR
   handles the `.js` glue but does not re-pull `web/wasm/`.

5. **Open DevTools console.** Expect the two log lines:

   ```
   [threads] main thread: SharedArrayBuffer=true crossOriginIsolated=true
   [sim] crossOriginIsolated=true
   [sim] rayon workers: N (hardware: M)
   ```

   If any of these say `false` or `1 thread`, the sim is single-threaded
   regardless of what your build looks like. Stop and fix the config
   drift — see the troubleshooting list below.

## Sim worker DevTools console

The sim worker's `[sim] ...` logs do **not** appear in the main page
console. Open them via:

- **Chrome / Edge**: DevTools → top-bar three-dot menu → **More tools →
  Threads/Workers**. Click the sim worker entry to attach.
- **Firefox**: `about:debugging#/runtime/this-firefox` → find the
  evosim tab → click **Inspect** next to the worker entry.

## Restarting cleanly

```bash
pkill -f 'vite.*47821'
sleep 1
# Then the start one-liner from step 3 above.
```

## What "rebuild wasm" means in different contexts

- **Cargo source changed (anything in `app/`)**: always rebuild.
- **`Cargo.toml` changed**: rebuild.
- **`.cargo/config.toml` changed**: rebuild, and verify both grep checks
  (the link args are what shared-memory hangs off).
- **Only `app/web/src/` or `app/web/index.html` changed**: skip wasm rebuild.
  Vite HMR is enough.
- **Only `docs/` changed**: nothing to rebuild or restart.

## Common failure modes

| Symptom | Likely cause | Fix |
|---|---|---|
| `creature_stride mismatch` or "X is not a function" runtime error | Stale wasm against new TS code | Rebuild wasm + hard-reload |
| `[boot] max_pop_for_sim mismatch: worker reported X, main expects Y` | Rust const and TS const drifted | Rebuild wasm (or fix the TS const if you just changed Rust) |
| `[sim] not cross-origin isolated; rayon disabled` | COOP/COEP missed somewhere | Check `web/vite.config.ts` AND `web/public/_headers` both set the headers |
| `[sim] rayon collapsed to 1 thread` | wasm bundle threaded but pool failed | Most often a header miss; sometimes a stale wasm |
| Sim looks frozen but main UI is responsive | Worker booted but tick loop is dark-holing | Check the sim-worker console for an error; restart the worker via `r` |
| All sliders / pause / TPS dropdown silently fail | `Atomics.waitAsync(0)` regression class | Run `pnpm test:e2e` — it catches this. Check the 1 ms floor in `simLoop` is intact |
| Vite refuses to start | Port 47821 is held | `lsof -i :47821`; kill the offender |
| Inexplicable Vite module errors | Vite cache stale (very rare) | `rm -rf web/node_modules/.vite` and restart |

## Stop conditions

If any of these happen, **halt and inspect** rather than retrying:

- `cargo build` succeeds but `wasm-pack build --features threads` fails
  with a link-time error → the `.cargo/config.toml` flags need
  attention.
- `grep -c initThreadPool` returns 0 even after `--features threads`
  → an upstream `wasm-bindgen-rayon` version moved or the feature flag
  isn't reaching the dep tree.
- The browser console shows `DataCloneError: ...postMessage...
  WebAssembly.Memory` → almost always `panic = "abort"` got dropped
  from the workspace `Cargo.toml`, or the linker emitted non-shared memory.

## See also

- [`index.md`](index.md)
- [`../architecture/build-and-deploy.md`](../architecture/build-and-deploy.md)
  — the canonical incantation + every flag's purpose.
- [`../architecture/worker-runtime.md`](../architecture/worker-runtime.md)
  — what those two `[sim] ...` log lines mean.
- [`testing-how-to.md`](testing-how-to.md) — running the gates between
  iterations.
- [`maintaining-docs.md`](maintaining-docs.md)
