# Deploy

## When does this apply

You need to ship a release build or understand how evosim is hosted on
**Cloudflare Pages**.

## The deploy path

Production is Cloudflare Pages, auto-deploying from the `main` branch on every
push. Cloudflare runs `bash app/cf-build.sh` and serves the static bundle.
The script compiles everything from source: it bootstraps a nightly Rust
toolchain, builds the threaded WASM via wasm-pack, asserts the
threaded-bundle invariants, provisions pnpm via `corepack` (CF's image ships
Node 20 but not pnpm), then runs `pnpm build` in `app/web/`. Nothing
prebuilt is committed — `app/web/wasm/` and `app/web/dist/` are gitignored.

`app/web/public/_headers` is copied to `dist/_headers` by Vite so Cloudflare
sets COOP/COEP/CORP + frame-ancestors + CSP natively at the edge on every
response (no service-worker shim required).

## Cloudflare Pages settings

| Field | Value |
|---|---|
| Root directory | *(leave blank — repo root)* |
| Build command | `bash app/cf-build.sh` |
| Build output directory | `app/web/dist` |

## Shipping a change

Push to `main` — Cloudflare auto-triggers a build using the settings above.
No manual steps required. The build log shows the wasm-pack output and the
two threaded-bundle grep checks; a failed check hard-aborts the script so
a broken bundle is never deployed.

## Preview the production bundle locally

```bash
bash app/cf-build.sh && ( cd app/web && pnpm preview )
```

This runs the full cold compile (takes several minutes; nightly + release
wasm-opt). Point a browser at `http://localhost:47821/`. The COOP/COEP
headers come from `vite preview` (not from `_headers`), which mirrors what
CF serves.

## What not to do

- Do NOT commit anything under `app/web/wasm/` or `app/web/dist/` —
  both are gitignored for good reason; the cold-compile model requires them
  to be built fresh on every deploy.
- Do NOT set `VITE_BASE` — Vite base defaults to `/` for Cloudflare Pages.
  The GitHub Pages sub-path model (`VITE_BASE=/evosim/`) is retired.
- Do NOT re-add the COI service-worker shim. Cloudflare Pages serves
  `_headers` natively; the shim is redundant and was deleted.
- Do NOT run `bash app/cf-build.sh` just to check TypeScript — use
  `cd app/web && pnpm typecheck` instead; it is much faster.

## See also

- [`../architecture/build-and-deploy.md`](../architecture/build-and-deploy.md) — WASM toolchain, COOP/COEP facts, link args, CI gates.
- [`../decisions/build.md`](../decisions/build.md) — why CF Pages + cold-compile-all; rationale for retiring GH Pages + COI shim.
- [`dev-loop.md`](dev-loop.md) — inner-loop wasm rebuild for local development.
