#!/usr/bin/env bash
# Cloudflare Pages deploy build — COMPILE-ALL path.
#
# Cloudflare rebuilds everything from source on every deploy: Rust→WASM (nightly,
# threaded, with atomics + shared-memory + build-std) via wasm-pack, then the
# Vite bundle. NOTHING is prebuilt or committed — app/web/wasm/ and app/web/dist/
# stay gitignored. Vite copies app/web/public/_headers to dist/_headers so the
# static host sets COOP/COEP/CORP + frame-ancestors natively (no service-worker
# shim needed).
#
# Cloudflare Pages settings:
#   Root directory:          (repo root — leave blank)
#   Build command:           bash app/cf-build.sh
#   Build output directory:  app/web/dist
#
# Preview the exact production bundle locally:
#   bash app/cf-build.sh && ( cd app/web && pnpm preview )
set -euo pipefail

APP="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"   # absolute path to app/
WEB="$APP/web"

# ── Rust / rustup ────────────────────────────────────────────────────────────
# Cloudflare's build image ships no Rust toolchain; bootstrap rustup when cargo
# is absent. On a dev box cargo is already on PATH and this block is skipped.
if ! command -v cargo >/dev/null 2>&1; then
  echo "==> Installing Rust (rustup)..."
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | \
    sh -s -- -y --default-toolchain none
  export PATH="$HOME/.cargo/bin:$PATH"
fi

# evosim pins stable via app/rust-toolchain.toml for native builds, but the
# threaded WASM requires nightly (atomics + build-std=panic_abort,std are
# still unstable on wasm32). Install/update nightly with rust-src so that
# the .cargo/config.toml [unstable] build-std directive works.
rustup toolchain install nightly --profile minimal --component rust-src 2>/dev/null || true
rustup target add wasm32-unknown-unknown --toolchain nightly 2>/dev/null || true

# ── wasm-pack ────────────────────────────────────────────────────────────────
# Install via the prebuilt-binary installer (fast, reliable on CI).
# Runs after the rustup bootstrap above so cargo is on PATH.
if ! command -v wasm-pack >/dev/null 2>&1; then
  echo "==> Installing wasm-pack..."
  curl -sSf https://rustwasm.github.io/wasm-pack/installer/init.sh | sh
  export PATH="$HOME/.cargo/bin:$PATH"
fi

# ── WASM build (nightly, threaded) ───────────────────────────────────────────
# out-dir is relative to the working directory (app/); ../../web/wasm → app/web/wasm/.
# .cargo/config.toml supplies the link args (--shared-memory, --import-memory,
# TLS exports) and [unstable] build-std — do NOT pass RUSTFLAGS here.
echo "==> Building threaded WASM (nightly, --features threads, --release)..."
( cd "$APP" && rustup run nightly wasm-pack build crates/evosim \
    --target web \
    --out-dir ../../web/wasm \
    --release \
    --features threads )

# ── Threaded-bundle invariant check ──────────────────────────────────────────
# Two grep checks that must both pass; failure means the build produced a
# single-threaded bundle (e.g. --features threads was omitted or the nightly
# link args were clobbered).
WASM_JS="$WEB/wasm/evosim.js"
INIT_COUNT="$(grep -c initThreadPool "$WASM_JS")"
if [ "$INIT_COUNT" -ne 2 ]; then
  echo "ERROR: threaded-bundle check FAILED: expected 2 'initThreadPool' occurrences, got $INIT_COUNT"
  echo "  The WASM bundle is NOT threaded. Check --features threads and the nightly toolchain."
  exit 1
fi
SHARED_COUNT="$(grep -cF 'shared:true' "$WASM_JS")"
if [ "$SHARED_COUNT" -lt 1 ]; then
  echo "ERROR: threaded-bundle check FAILED: 'shared:true' not found in $WASM_JS"
  echo "  SharedArrayBuffer memory is not wired. Check --shared-memory link arg in .cargo/config.toml."
  exit 1
fi
echo "==> Threaded-bundle checks passed (initThreadPool: $INIT_COUNT, shared:true: $SHARED_COUNT)"

# ── pnpm (Cloudflare's image has Node but no pnpm; a stale asdf shim shadows it) ──
# corepack ships with Node 20 and is the official provisioning mechanism.
# corepack writes the pnpm shim into Node's REAL install bin dir; we must put that
# dir ahead of asdf's stale `pnpm` shim so the corepack shim wins. Guard makes this
# a no-op on the dev box (pnpm already present).
export COREPACK_ENABLE_DOWNLOAD_PROMPT=0
if ! pnpm --version >/dev/null 2>&1; then
  echo "==> Provisioning pnpm via corepack..."
  # Cloudflare manages Node via asdf, so `command -v node` is the asdf shim
  # (~/.asdf/shims/node), NOT the real bin. process.execPath resolves the real
  # node binary (asdf's shim execs into it); its dir is where corepack writes the
  # pnpm shim. Prepend it BEFORE invoking corepack so asdf's stale `pnpm` shim
  # (which errors "No preset version installed for command pnpm", exit 126) loses.
  NODE_BIN_DIR="$(dirname "$(node -e 'process.stdout.write(process.execPath)')")"
  export PATH="$NODE_BIN_DIR:$PATH"
  corepack enable
  corepack prepare pnpm@10.33.4 --activate
  pnpm --version
fi

# ── Web bundle (pnpm + vite) ──────────────────────────────────────────────────
# pnpm build runs: tsc --noEmit && vite build
# No VITE_BASE — vite base defaults to "/" for Cloudflare Pages.
echo "==> Building web bundle (pnpm install + pnpm build)..."
( cd "$WEB" && pnpm install --frozen-lockfile && pnpm build )

echo "==> Done — output in $WEB/dist"
ls -la "$WEB/dist"
