#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
WEB_DIR="$ROOT/web"
PORT=47821

run() {
  printf '\n==> %s\n' "$*"
  "$@"
}

require_cmd() {
  if ! command -v "$1" >/dev/null 2>&1; then
    printf 'missing required command: %s\n' "$1" >&2
    exit 127
  fi
}

# Kill any process currently listening on PORT so `pnpm preview --strictPort`
# does not fail when a previous run was left behind.
free_port() {
  local port="$1"
  local pids

  if command -v lsof >/dev/null 2>&1; then
    pids="$(lsof -ti "tcp:${port}" -sTCP:LISTEN 2>/dev/null)" || true
  elif command -v fuser >/dev/null 2>&1; then
    pids="$(fuser "${port}/tcp" 2>/dev/null | tr -s ' ' '\n' | grep -E '^[0-9]+$')" || true
  else
    pids=""
  fi

  [[ -z "$pids" ]] && return 0

  printf '\n==> port %s held by PID(s) %s — sending SIGTERM\n' "$port" "$pids"
  # shellcheck disable=SC2086
  kill -TERM $pids 2>/dev/null || true

  # Poll up to 3 s for the port to free; escalate to SIGKILL if needed.
  local i
  for i in 1 2 3; do
    sleep 1
    if command -v lsof >/dev/null 2>&1; then
      pids="$(lsof -ti "tcp:${port}" -sTCP:LISTEN 2>/dev/null)" || true
    elif command -v fuser >/dev/null 2>&1; then
      pids="$(fuser "${port}/tcp" 2>/dev/null | tr -s ' ' '\n' | grep -E '^[0-9]+$')" || true
    else
      pids=""
    fi
    [[ -z "$pids" ]] && return 0
  done

  printf '==> port %s still held — escalating to SIGKILL\n' "$port"
  # shellcheck disable=SC2086
  kill -KILL $pids 2>/dev/null || true
  sleep 1
}

require_cmd cargo
require_cmd rustup
require_cmd wasm-pack
require_cmd pnpm

cd "$ROOT"

# Build the wasm with the OPTIMIZED release profile by default (opt-level=3 +
# thin LTO + codegen-units=1 from [profile.release], then wasm-opt -O4 from the
# crate's wasm-pack release metadata). The unoptimized `--dev` profile leaves
# the compute-heavy sim phases (NN forward, grass scatter) several times slower,
# so local perf no longer matches production. Set DEV_WASM=1 for a fast
# (~3s vs ~45s) unoptimized build when iterating on non-perf-sensitive changes.
if [[ "${DEV_WASM:-0}" == "1" ]]; then
  WASM_PROFILE_FLAG="--dev"
  printf '\n==> wasm profile: DEV (unoptimized — set DEV_WASM=0 for optimized)\n'
else
  WASM_PROFILE_FLAG="--release"
  printf '\n==> wasm profile: RELEASE (optimized — set DEV_WASM=1 for fast builds)\n'
fi

# Keep the dev compile gate to library/binary targets. `--all-targets` also
# builds Criterion benches through `cargo build`, which mixes the workspace's
# dev `panic=abort` artifacts with the bench harness' required `panic=unwind`.
run cargo build --workspace --lib --bins
run cargo build --workspace --lib --bins --features threads
run cargo run --bin gen-bindings

run rm -rf "$WEB_DIR/dist" "$WEB_DIR/wasm"
run rustup run nightly wasm-pack build crates/evosim \
  --target web \
  --out-dir ../../web/wasm \
  "$WASM_PROFILE_FLAG" \
  --features threads

init_thread_pool_count="$(grep -c initThreadPool "$WEB_DIR/wasm/evosim.js" || true)"
if [[ "$init_thread_pool_count" == "0" ]]; then
  printf 'threaded wasm verification failed: initThreadPool not found in web/wasm/evosim.js\n' >&2
  exit 1
fi

if ! grep -F 'shared:true' "$WEB_DIR/wasm/evosim.js" >/dev/null; then
  printf 'threaded wasm verification failed: shared:true not found in web/wasm/evosim.js\n' >&2
  exit 1
fi

cd "$WEB_DIR"
run pnpm build

free_port "$PORT"
printf '\n==> serving built app at http://localhost:%s/ (Ctrl-C to stop)\n' "$PORT"
exec pnpm preview --host 0.0.0.0
