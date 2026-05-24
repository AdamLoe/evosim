# Build & Tooling Audit

Scope: `Cargo.toml`, `.cargo/config.toml`, `web/{package,vite.config,tsconfig}.json/ts`,
`.github/workflows/ci.yml`. Reviewed 2026-05-24.

Summary: release profile is reasonable; CI covers fmt/clippy/test/threaded golden
+ wasm-pack build. Main gaps: **no wasm-opt**, **no wasm size budget**, **no
benchmark harness / perf regression tracking**, **no toolchain/node pinning
files**, **no web lint**, **sourcemap=true in release web build**, no
pre-commit hooks.

---

## 1. Release profile — mostly good, two tweaks

Current `Cargo.toml`:

```toml
[profile.release]
opt-level = 3
lto = "thin"
codegen-units = 1
panic = "abort"
```

Strong points: `codegen-units = 1`, `panic = "abort"`, single crate so LTO is
effective. Two missing knobs:

- **`lto = "fat"`** — for a single-crate cdylib targeting wasm, fat LTO is
  typically a few % smaller and faster than thin. Worth a measured swap.
- **`strip = "debuginfo"`** — Rust 1.59+. Current release wasm carries debug
  symbols (no `strip`, no `-C debuginfo=0`); the 431 KB `evosim_bg.wasm`
  likely has 50–100 KB of strippable name section / DWARF noise.
- **`overflow-checks = false`** (default for release, fine — call out
  explicitly for inner-loop sims) — leave as-is, but document.

Recommended:

```toml
[profile.release]
opt-level = 3
lto = "fat"
codegen-units = 1
panic = "abort"
strip = "debuginfo"   # keep "symbols" off so backtraces survive

[profile.release.package."*"]
opt-level = 3         # ensure deps also build at -O3 (default, just explicit)
```

Optional: a dedicated `profile.wasm-release` inheriting from release, used by
`wasm-pack --profile wasm-release`, so native `cargo test --release` doesn't
pay fat-LTO cost.

---

## 2. wasm-opt — not configured

Neither `Cargo.toml` `[package.metadata.wasm-pack.profile.release]` nor a
CI step runs `wasm-opt`. wasm-pack ships an internal wasm-opt but only if
the metadata block opts in. Without it, the published `web/wasm/evosim_bg.wasm`
is the raw rustc output.

Add to `Cargo.toml`:

```toml
[package.metadata.wasm-pack.profile.release]
wasm-opt = ["-O4", "--enable-bulk-memory", "--enable-threads",
            "--enable-mutable-globals", "--strip-debug"]

# Re-enable a sanity profile that *doesn't* wasm-opt for fast local rebuilds:
[package.metadata.wasm-pack.profile.dev]
wasm-opt = false
```

Threads build is using `+atomics,+bulk-memory,+mutable-globals` — wasm-opt
must be invoked with matching `--enable-*` flags or it will reject the
module. The flags above match `.cargo/config.toml`.

Expected: 20–30 % size reduction and a few % runtime gain.

---

## 3. Wasm size budget — none

`web/wasm/evosim_bg.wasm` is 431 KB today. No CI gate prevents drift.

Add a CI step after `wasm-pack build`:

```yaml
      - name: wasm size budget
        run: |
          BYTES=$(stat -c%s web/wasm/evosim_bg.wasm)
          echo "wasm size: $BYTES bytes"
          test "$BYTES" -lt 524288  # 512 KiB budget (raise once wasm-opt lands)
```

Couple this with a per-PR comment (e.g. `actions/github-script`) to track
trend instead of just a hard gate.

---

## 4. CI — solid baseline, missing pieces

`.github/workflows/ci.yml` already runs: `fmt --check`, `clippy -D warnings`,
`cargo test --lib`, `cargo test --release --test acceptance` (single-thread
and `--features threads` golden), wasm-pack release build, `pnpm build`
(which runs `tsc --noEmit` via the `build` script). Good coverage of the
fundamentals.

Gaps:

1. **No `cargo test --release` for the non-acceptance suites.** Some
   determinism bugs only surface at `-O3`. Add:
   ```yaml
   - name: test (release, all)
     run: cargo test --release
   ```
   Acceptance test already does `--release`, but the lib tests run at dev
   opt-level=1.

2. **Clippy on threads feature.** Current clippy run uses default features,
   so the `threads`-gated code never gets linted.
   ```yaml
   - name: clippy (threads)
     run: cargo clippy --all-targets --features threads -- -D warnings
   ```

3. **No `cargo deny` / `cargo audit`.** Supply-chain regression risk for
   `wasm-bindgen`, `rand`, `getrandom` is real.
   ```yaml
   - uses: EmbarkStudios/cargo-deny-action@v2
   ```

4. **No web lint.** `package.json` has `typecheck` but no ESLint /
   `eslint-plugin-import` / `tsc --strict` cross-check on tests. Either add
   an ESLint config or accept that `tsc` (which IS strict via `tsconfig.json`)
   is the only check. Recommend adding biome or eslint:
   ```json
   "scripts": {
     "lint": "eslint src --max-warnings 0",
     ...
   }
   ```
   then `pnpm lint` in CI.

5. **No threaded-build wasm size gate** (paired with #3).

6. **No Cloudflare Pages preview deploy** — every PR should publish a
   preview so the `CODE READY` UI items in BUILD-REPORT.md actually get
   eyeballed. Trivial via Cloudflare's GitHub action.

---

## 5. Benchmark harness — missing

No `benches/`, no `criterion`, no recorded perf history. BUILD-REPORT.md
already cites "2.31 s on dev machine" for the acceptance run with an 8 s
budget — that ratio is the only perf signal in the repo, and it lives in
prose.

Recommendation: ship Criterion benches for the hot paths the recent
commits (1844f78 grid cursor, c382916 scratch pool, cbb410e sector trig,
8f8d202 SoA split) just optimized:

```toml
# Cargo.toml
[dev-dependencies]
criterion = { version = "0.5", default-features = false, features = ["html_reports"] }

[[bench]]
name = "tick"
harness = false
```

`benches/tick.rs`: world at N=500, N=1500 creatures, bench `World::step()`.
Wire to CI behind `cargo bench --no-run` (compile-only) plus an
optional-on-main `bencher.dev` / `cargo-criterion` upload. At minimum: a
nightly job that runs criterion and posts a comment on regression.

Also add a coarser **wall-clock acceptance perf gate** that fails on >25 %
regression vs a checked-in baseline:

```rust
// tests/acceptance.rs (already has the 8s budget, tighten + record)
const ACCEPTANCE_TICK_BUDGET_MS: f64 = 4_000.0;  // was implicit 8000
```

---

## 6. Reproducibility — soft

- **No `rust-toolchain.toml`.** CI uses `dtolnay/rust-toolchain@stable`,
  meaning Tuesday's rustc release can break Wednesday's PR. Pin:
  ```toml
  # rust-toolchain.toml
  [toolchain]
  channel = "1.83.0"             # current stable as of 2026-05
  components = ["rustfmt", "clippy", "rust-src"]
  targets   = ["wasm32-unknown-unknown"]
  ```
  This also removes the dual `rustup toolchain install nightly` dance in
  CI — but note `[unstable] build-std` in `.cargo/config.toml` *requires*
  nightly for the wasm-thread build. Either keep a pinned nightly
  (`nightly-2026-05-01`) for that path, or migrate off `build-std` once
  std ships pre-built atomics for `wasm32-unknown-unknown` (not yet on
  stable).

- **No `.nvmrc` / `engines` in `package.json`.** CI hard-codes Node 20,
  pnpm 10; local devs can drift. Add:
  ```
  // web/.nvmrc
  20.18.0
  ```
  and to `web/package.json`:
  ```json
  "engines": { "node": ">=20.10 <21", "pnpm": ">=10 <11" },
  "packageManager": "pnpm@10.0.0"
  ```

- **`Cargo.lock` committed** — good (it is). Confirm it's not in
  `.gitignore` (it isn't; only `Cargo.lock.bak`).

- **`pnpm-lock.yaml` committed** — good (it is). CI uses
  `--frozen-lockfile` — good.

- **`getrandom` features `["js"]`** is correct for wasm; no concern.

---

## 7. Vite / TS — minor

- `vite.config.ts` has `sourcemap: true` for the **build** target. Ships
  `.js.map` to Cloudflare for production users. Either set to `false` for
  prod, or `"hidden"` (emit map but no `//# sourceMappingURL` comment) so
  Sentry/error tooling can fetch it but browsers don't auto-pull.
  ```ts
  build: {
    sourcemap: process.env.CI ? "hidden" : true,
    ...
  }
  ```

- `tsconfig.json` is strict and clean. Consider adding
  `"noUncheckedIndexedAccess": true` (the sim code does a lot of indexed
  buffer access from wasm — this catches off-by-ones early).

- No `tsc` check on the `wasm/` folder's generated `.d.ts` — Vite handles
  it but `tsc --noEmit` should already cover it via the `include` array.
  Verified: `tsconfig.json` includes `"wasm"`. Good.

---

## 8. Pre-commit hooks — none

`.git/hooks/` is sample-only. With CI doing fmt+clippy+typecheck,
pre-commit is optional, but a fast local guard removes a CI round-trip.

Minimal `.husky/pre-commit` (or `lefthook`/raw shell):

```bash
#!/usr/bin/env bash
set -euo pipefail
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
( cd web && pnpm typecheck )
```

Or wire via `lefthook.yml` checked into the repo. Skip if local-dev speed
matters more than fast-feedback.

---

## Priority order

1. **Add `wasm-opt` metadata block** (1-line config, ~25 % size win).
2. **Pin toolchain** (`rust-toolchain.toml`, `.nvmrc`, `packageManager`).
3. **Wasm size budget** in CI.
4. **Criterion bench harness** for the perf hot paths (recent commits make
   this acute — there's now no way to detect a regression to the SoA /
   grid-cursor / sector-trig wins).
5. **`profile.release.strip = "debuginfo"` + `lto = "fat"`**, measure.
6. **`cargo test --release` (full suite)** + **clippy with `--features threads`**.
7. **`cargo deny` / `cargo audit`** in CI.
8. **`sourcemap: "hidden"`** in prod vite build.
9. Optional: ESLint/biome, pre-commit, Cloudflare preview deploys.

## Key file paths

- `/home/adamg/evosim/Cargo.toml`
- `/home/adamg/evosim/.cargo/config.toml`
- `/home/adamg/evosim/.github/workflows/ci.yml`
- `/home/adamg/evosim/web/package.json`
- `/home/adamg/evosim/web/vite.config.ts`
- `/home/adamg/evosim/web/tsconfig.json`
- `/home/adamg/evosim/web/public/_headers`
- `/home/adamg/evosim/tests/acceptance.rs` (existing perf gate)
- `/home/adamg/evosim/tests/golden_snapshot_t10000{,_threaded}.txt`
