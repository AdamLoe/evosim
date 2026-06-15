# Decisions — build

---

### Threaded wasm build is the default; plain build is for testing the cfg path only

- **Decision**: Every dev and release wasm build uses
  `--features threads`. The plain build (`wasm-pack build crates/evosim
  --target web --out-dir ../../web/wasm --dev`) exists only to exercise
  the `cfg(not(feature = "threads"))` Rust paths.
- **Why**: The plain build runs ~1/N× speed with no error or warning —
  rayon silently collapses to 1 thread. The footgun has bitten the
  project enough times that the default rule needs to be loud.
- **Tradeoffs**: Threaded build needs a nightly toolchain. Cost is one
  `rustup run nightly` prefix.
- **Applies to**: `architecture/build-and-deploy.md`,
  `agent-context/dev-loop.md`.
- **Code anchors**: `docs/architecture/build-and-deploy.md`
  (canonical incantation), `.github/workflows/ci.yml`
  (CI uses `--features threads` for the wasm build step).

### `panic = "abort"` on both `dev` and `release` profiles

- **Decision**: `Cargo.toml` sets `panic = "abort"` in both profiles.
- **Why**: Required when building `--features threads` for wasm32.
  Without it, the linker silently emits non-shared
  `WebAssembly.Memory` and `wasm-bindgen-rayon`'s
  `postMessage(memory)` fails with `DataCloneError` at boot.
- **Applies to**: `architecture/build-and-deploy.md`.
- **Code anchors**: `Cargo.toml → [profile.dev]` /
  `[profile.release]`.

### Link args live in `.cargo/config.toml`, not in `RUSTFLAGS` env

- **Decision**: All wasm-only link args live under
  `[target.wasm32-unknown-unknown] rustflags` in `.cargo/config.toml`.
  Builds must not override via `RUSTFLAGS=...` on the command line.
- **Why**: Setting `RUSTFLAGS` env *overrides* the config file, silently
  dropping the link args. The build "succeeds" but produces a broken
  bundle.
- **Code anchors**: `.cargo/config.toml`,
  `architecture/build-and-deploy.md`.
- **Applies to**: `architecture/build-and-deploy.md`.

### Explicit `--shared-memory` link arg

- **Decision**: `.cargo/config.toml` passes
  `-C link-arg=--shared-memory` to the wasm linker.
- **Why**: On recent nightlies, `+atomics` no longer implies
  `--shared-memory`. Without the explicit arg, wasm-ld emits
  non-shared memory and `wasm-bindgen-rayon` fails to clone it across
  workers.
- **Applies to**: `architecture/build-and-deploy.md`.
- **Code anchors**: `.cargo/config.toml`.
- **Revisit when**: nightly stabilizes the inference and the explicit
  arg becomes redundant (don't drop it speculatively — verify with a
  clean build + the two grep checks first).

### COOP/COEP set in two places — must stay in sync

- **Decision**: `Cross-Origin-Opener-Policy: same-origin` and
  `Cross-Origin-Embedder-Policy: require-corp` are set in BOTH
  `web/vite.config.ts` (dev + preview) and `web/public/_headers`
  (Cloudflare-Pages-style static deploy).
- **Why**: Without both, the sim "looks fine" but `crossOriginIsolated`
  is false somewhere — main or worker silently collapses to 1 rayon
  thread.
- **Tradeoffs**: Two-place duplication; the dev and deploy paths
  legitimately need independent config. The risk is drift.
- **Applies to**: `architecture/build-and-deploy.md`,
  `architecture/worker-runtime.md`.
- **Code anchors**: `web/vite.config.ts → crossOriginIsolationHeaders`,
  `web/public/_headers`.

### GitHub Pages deploy sets `VITE_BASE` and uses the COI shim

- **Decision**: `.github/workflows/deploy-pages.yml` sets
  `VITE_BASE=/evosim/` for the Pages build. GitHub Pages cannot serve the
  `_headers` COOP/COEP file, so `web/index.html` loads the COI
  service-worker shim before the app module.
- **Why**: Built asset URLs must resolve under the repository path on
  Pages, and threaded wasm still needs cross-origin isolation on hosts that
  cannot set response headers.
- **Applies to**: `architecture/build-and-deploy.md`.
- **Code anchors**: `.github/workflows/deploy-pages.yml`,
  `web/vite.config.ts → base`, `web/index.html`.

### `web/wasm/` artifacts are gitignored and never auto-rebuilt

- **Decision**: `web/wasm/.gitignore` ignores wasm-pack outputs.
  `pnpm dev` does NOT invoke `wasm-pack`. Every Rust change requires a manual
  `wasm-pack build ...` and a hard browser reload.
- **Why**: The wasm-pack outputs are large binary artifacts whose
  diffs are unreviewable; checking them in would balloon the repo and
  invite stale-artifact bugs. Tying `pnpm dev` to a Rust rebuild would
  conflate Rust- and TS-iteration loops; some agents want to iterate
  in TS without paying the Rust build cost.
- **Tradeoffs**: New contributors hit "stride mismatch" type errors
  the first time they pull a Rust change without rebuilding. Mitigated
  by the runtime guard in `render/gl.ts` (`stride !== 8 → throw`) and
  the boot handshake assert (`max_pop_for_sim` mismatch → throw with a
  rebuild-wasm message).
- **Applies to**: `architecture/build-and-deploy.md`,
  `agent-context/dev-loop.md`.
- **Code anchors**: `web/wasm/.gitignore`.

### Vite `worker: { format: "es" }`

- **Decision**: `web/vite.config.ts` sets `worker: { format: "es" }`.
- **Why**: `wasm-bindgen-rayon` spawns workers via `new
  URL('./workerHelpers.js', import.meta.url)`. Vite must bundle them
  as ES modules; the default IIFE format causes `pnpm build` to fail
  with "UMD and IIFE output formats are not supported for
  code-splitting builds."
- **Applies to**: `architecture/build-and-deploy.md`,
  `architecture/worker-runtime.md`.
- **Code anchors**: `web/vite.config.ts`.

### CI runs the threaded-feature gate as a first-class step

- **Decision**: `.github/workflows/ci.yml` includes
  `cargo clippy --all-targets --features threads -- -D warnings` AND
  `cargo test --lib --features threads` AND a release wasm build —
  not as an optional matrix entry, as required steps.
- **Why**: Per-feature breakage at the threaded gate has shipped
  multiple times under different `cfg` paths. Making them first-class
  means a PR that breaks them fails the gate.
- **Applies to**: `architecture/build-and-deploy.md`,
  `architecture/testing.md`.
- **Code anchors**: `.github/workflows/ci.yml`.

### Port pinned to 47821 with `strictPort: true`

- **Decision**: `web/vite.config.ts` pins `port: 47821, strictPort:
  true`. Vite refuses to start if 47821 is held instead of silently
  picking a different port.
- **Why**: Hard failures are debuggable; silent reassignment is not. A
  background-server pattern that assumes 47821 breaks invisibly under
  reassignment.
- **Applies to**: `architecture/build-and-deploy.md`,
  `agent-context/dev-loop.md`.

## See also

- [`../architecture/build-and-deploy.md`](../architecture/build-and-deploy.md)
- [`../architecture/worker-runtime.md`](../architecture/worker-runtime.md)
- [`../agent-context/dev-loop.md`](../agent-context/dev-loop.md)
