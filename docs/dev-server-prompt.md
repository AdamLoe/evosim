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

> Before starting, make sure the wasm artifact exists. If
> `web/wasm/evosim_bg.wasm` is missing or stale, build it first (see
> section 7).

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

If the browser console shows a fetch error for `evosim_bg.wasm`, or the file
is missing, rebuild from the repo root:

```bash
cd /home/adamg/evosim
wasm-pack build --target web --out-dir web/wasm --dev
```

Then restart the dev server (section 5) so Vite picks up the new artifact.

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
| Rail UI (speed, controls) | `web/src/rail/*.ts` |
| Eulogy / end-of-run overlay | `web/src/eulogy/*` |
| Persistence / save-load | `web/src/persistence/*` |
| Simulation logic (Rust) | `src/*.rs` |
| Wasm API surface | `src/wasm_api.rs` (`WorldHandle`) |

Hot-reload is active: edit a `.ts` file and the browser refreshes
automatically. Rust changes require a `wasm-pack build …` followed by a
manual browser reload (or server restart).

---

## 9. Tail recent server logs

```bash
tail -50 /tmp/vite.log
```

Full log since last start is always at `/tmp/vite.log`. The file is wiped on
each `rm -f /tmp/vite.log` in the start command (section 3), so it only
contains output from the current run.
